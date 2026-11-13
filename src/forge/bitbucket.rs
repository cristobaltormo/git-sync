use std::sync::Arc;

use serde::Deserialize;
use serde::de::IgnoredAny;
use serde_json::{Value, json};

use super::common::{
    Common, Text, check, cut, decode_with, nullable, path_escape, rejected, tolerant,
};
use super::gitbucket::NO_GLOBAL_HOOKS;
use super::{Change, Event, Hooks, Identity, PageCache, Provider, Repo, Source, valid_mac};
use crate::config::Config;
use crate::error::Error;
use crate::httpx::{Client, Headers, Response, fail};

pub struct BitbucketCloud {
    c: Common,
    base: String,
    http: Client,
    pages: PageCache,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct Branch<'a> {
    #[serde(borrow)]
    name: Text<'a>,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct BbRepo<'a> {
    #[serde(borrow)]
    uuid: Text<'a>,
    #[serde(borrow)]
    slug: Text<'a>,
    #[serde(borrow)]
    full_name: Text<'a>,
    #[serde(deserialize_with = "nullable")]
    is_private: bool,
    #[serde(borrow)]
    description: Text<'a>,
    #[serde(borrow)]
    website: Text<'a>,
    #[serde(borrow)]
    updated_on: Text<'a>,
    parent: Option<IgnoredAny>,
    #[serde(borrow)]
    mainbranch: Option<Branch<'a>>,
}

pub(crate) fn hash_id(s: &str) -> i64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in s.bytes() {
        h ^= u64::from(b);
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    (h >> 1) as i64
}

impl BbRepo<'_> {
    fn into_repo(self) -> Repo {
        Repo {
            id: hash_id(self.uuid.as_str()),
            owner: cut(self.full_name.as_str(), '/').0.to_string(),
            name: self.slug.into_string(),
            private: self.is_private,
            description: self.description.trimmed(),
            website: self.website.trimmed(),
            fork: self.parent.is_some(),
            updated: self.updated_on.into_string(),
            empty: self.mainbranch.is_none(),
            default_branch: self
                .mainbranch
                .map(|b| b.name.into_string())
                .unwrap_or_default(),
            ..Default::default()
        }
    }
}

fn non_empty(b: &[u8]) -> Result<&[u8], Error> {
    if b.is_empty() {
        return Err(Error::other("empty response"));
    }
    Ok(b)
}

fn warn_removal(r: &Repo, http: &Client, url: &str) -> bool {
    let err = match http.request("DELETE", url, None) {
        Ok(d) if d.ok() => return true,
        Ok(d) => fail(&d, ""),
        Err(e) => e,
    };
    crate::warnf!("{}: removing webhook: {err}", r.full());
    false
}

impl BitbucketCloud {
    pub fn new(cfg: &Config) -> BitbucketCloud {
        let c = Common::new(cfg);
        let base = if c.url == "https://bitbucket.org" {
            "https://api.bitbucket.org/2.0".to_string()
        } else {
            format!("{}/2.0", c.url)
        };
        BitbucketCloud {
            base,
            http: c.client("Bearer"),
            pages: PageCache::default(),
            c,
        }
    }

    fn fetch(&self, path: &str) -> Result<Response, Error> {
        self.http.get(&format!("{}{path}", self.base))
    }

    fn path(r: &Repo) -> String {
        format!(
            "/repositories/{}/{}/hooks",
            path_escape(&r.owner),
            path_escape(&r.name)
        )
    }

    fn hook_body(&self) -> Value {
        json!({
            "description": "gitsync",
            "url": self.c.hook_url,
            "active": true,
            "secret": self.c.secret,
            "events": ["repo:push", "repo:updated"],
        })
    }

    fn ours(&self, r: &Repo) -> Result<Vec<String>, Error> {
        let resp = check(self.fetch(&format!("{}?pagelen=100", Self::path(r)))?)?;
        #[derive(Deserialize, Default)]
        #[serde(default)]
        struct H<'a> {
            #[serde(borrow)]
            uuid: Text<'a>,
            #[serde(borrow)]
            url: Text<'a>,
        }
        #[derive(Deserialize, Default)]
        #[serde(default)]
        struct Page<'a> {
            #[serde(borrow, deserialize_with = "nullable")]
            values: Vec<H<'a>>,
        }
        let page: Page<'_> = resp.decode()?;
        Ok(page
            .values
            .into_iter()
            .filter(|x| x.url.as_str() == self.c.hook_url)
            .map(|x| x.uuid.into_string())
            .collect())
    }
}

impl Source for BitbucketCloud {
    fn kind(&self) -> &'static str {
        "bitbucket-cloud"
    }

    fn whoami(&self) -> Result<Identity, Error> {
        let r = self.fetch("/user")?;
        if r.ok() {
            #[derive(Deserialize, Default)]
            #[serde(default)]
            struct U<'a> {
                #[serde(borrow)]
                nickname: Text<'a>,
                #[serde(borrow)]
                username: Text<'a>,
            }
            let u: U<'_> = r.decode()?;
            let login = if u.nickname.is_empty() {
                u.username
            } else {
                u.nickname
            };
            return Ok(Identity {
                login: login.into_string(),
                ..Default::default()
            });
        }
        let r = self.fetch("/repositories?role=member&pagelen=1")?;
        if !r.ok() {
            return Err(Error::api(r.status, "the source token was rejected"));
        }
        Ok(Identity {
            login: "token".into(),
            ..Default::default()
        })
    }

    fn list(&self, owner: &str) -> Result<Vec<Arc<Repo>>, Error> {
        let mut next = format!(
            "{}/repositories/{}?pagelen=100",
            self.base,
            path_escape(owner)
        );
        let inside = format!("{}/", self.base);
        let mut out = Vec::new();
        while !next.is_empty() {
            if !next.starts_with(&inside) {
                return Err(crate::err!(
                    "listing {owner}: refusing to follow a next page outside {}",
                    self.base
                ));
            }
            let r = self.http.get(&next)?;
            if !r.ok() {
                return Err(fail(&r, &format!("listing {owner}")));
            }
            #[derive(Deserialize, Default)]
            #[serde(default)]
            struct Page<'a> {
                #[serde(borrow, deserialize_with = "nullable")]
                values: Vec<BbRepo<'a>>,
                #[serde(borrow)]
                next: Text<'a>,
            }
            let (rows, more) = self.pages.decode_meta(&next, &r.body, |b| {
                tolerant(non_empty(b)?, |b| {
                    let page: Page<'_> = serde_json::from_slice(b)?;
                    let rows = page.values.into_iter().map(BbRepo::into_repo).collect();
                    Ok((rows, page.next.into_string()))
                })
            })?;
            out.extend(rows.iter().cloned());
            next = more;
        }
        Ok(out)
    }

    fn get(&self, owner: &str, name: &str) -> Result<Option<Arc<Repo>>, Error> {
        let r = self.fetch(&format!(
            "/repositories/{}/{}",
            path_escape(owner),
            path_escape(name)
        ))?;
        if r.status == 404 {
            return Ok(None);
        }
        let r = check(r)?;
        let repo = decode_with(&r, |b| {
            Ok(serde_json::from_slice::<BbRepo<'_>>(b)?.into_repo())
        })?;
        Ok(Some(Arc::new(repo)))
    }

    fn owner_exists(&self, owner: &str) -> Result<bool, Error> {
        Ok(self
            .fetch(&format!("/workspaces/{}", path_escape(owner)))?
            .ok())
    }

    fn hooks(&self) -> Option<&dyn Hooks> {
        Some(self)
    }
}

impl Provider for BitbucketCloud {
    fn clone_url(&self, r: &Repo) -> String {
        format!("{}/{}/{}.git", self.c.url, r.owner, r.name)
    }

    fn git_header(&self) -> Result<String, Error> {
        Ok(self.c.basic_header("x-token-auth"))
    }

    fn parse_webhook(
        &self,
        h: &Headers,
        body: &[u8],
        secret: &str,
    ) -> Result<Option<Event>, Error> {
        if !valid_mac(secret, body, h.get("X-Hub-Signature"), "sha256=") {
            return Err(Error::Signature);
        }
        if !h.get("X-Event-Key").starts_with("repo:") {
            return Ok(None);
        }
        #[derive(Deserialize, Default)]
        #[serde(default)]
        struct R<'a> {
            #[serde(borrow)]
            uuid: Text<'a>,
            #[serde(borrow)]
            full_name: Text<'a>,
        }
        #[derive(Deserialize, Default)]
        #[serde(default)]
        struct P<'a> {
            #[serde(borrow)]
            repository: Option<R<'a>>,
        }
        tolerant(body, |b| {
            let p: P<'_> = serde_json::from_slice(b)?;
            let repo = p.repository.unwrap_or_default();
            let Some((owner, name)) = repo.full_name.as_str().split_once('/') else {
                return Ok(None);
            };
            Ok(Some(Event {
                kind: Change::Changed,
                owner: owner.to_string(),
                name: name.to_string(),
                id: hash_id(repo.uuid.as_str()),
            }))
        })
    }
}

impl Hooks for BitbucketCloud {
    fn global(&self, _known: i64, _force: bool) -> Result<(i64, String), Error> {
        Err(Error::other(NO_GLOBAL_HOOKS))
    }

    fn repo(&self, r: &Repo, force: bool) -> Result<String, Error> {
        let mine = self.ours(r)?;
        if !mine.is_empty() && !force {
            return Ok("exists".into());
        }
        let path = Self::path(r);
        let (method, target, result) = match mine.first() {
            Some(uuid) => (
                "PUT",
                format!("{}{path}/{}", self.base, path_escape(uuid)),
                "updated",
            ),
            None => ("POST", format!("{}{path}", self.base), "created"),
        };
        check(self.http.json(method, &target, &self.hook_body())?)?;
        Ok(result.into())
    }

    fn remove_global(&self, _id: i64) -> bool {
        false
    }

    fn remove_repo(&self, r: &Repo) -> i64 {
        let mine = match self.ours(r) {
            Ok(m) => m,
            Err(e) => {
                crate::warnf!("{}: listing webhooks: {e}", r.full());
                return 0;
            }
        };
        let path = Self::path(r);
        mine.iter()
            .filter(|uuid| {
                let url = format!("{}{path}/{}", self.base, path_escape(uuid));
                warn_removal(r, &self.http, &url)
            })
            .count() as i64
    }
}

pub struct BitbucketServer {
    c: Common,
    base: String,
    http: Client,
    pages: PageCache,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct ProjectKey<'a> {
    #[serde(borrow)]
    key: Text<'a>,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct BsRepo<'a> {
    #[serde(deserialize_with = "nullable")]
    id: i64,
    #[serde(borrow)]
    slug: Text<'a>,
    #[serde(borrow)]
    description: Text<'a>,
    #[serde(deserialize_with = "nullable")]
    public: bool,
    #[serde(deserialize_with = "nullable")]
    archived: bool,
    origin: Option<IgnoredAny>,
    #[serde(borrow)]
    project: Option<ProjectKey<'a>>,
}

impl BsRepo<'_> {
    fn into_repo(self) -> Repo {
        Repo {
            id: self.id,
            owner: self
                .project
                .map(|p| p.key.into_string())
                .unwrap_or_default(),
            name: self.slug.into_string(),
            private: !self.public,
            description: self.description.trimmed(),
            archived: self.archived,
            fork: self.origin.is_some(),
            ..Default::default()
        }
    }
}

impl BitbucketServer {
    pub fn new(cfg: &Config) -> BitbucketServer {
        let c = Common::new(cfg);
        BitbucketServer {
            base: format!("{}/rest/api/1.0", c.url),
            http: c.client("Bearer"),
            pages: PageCache::default(),
            c,
        }
    }

    fn fetch(&self, path: &str) -> Result<Response, Error> {
        self.http.get(&format!("{}{path}", self.base))
    }

    fn path(r: &Repo) -> String {
        format!(
            "/projects/{}/repos/{}/webhooks",
            path_escape(&r.owner),
            path_escape(&r.name)
        )
    }

    fn hook_body(&self) -> Value {
        json!({
            "name": "gitsync",
            "url": self.c.hook_url,
            "active": true,
            "events": ["repo:refs_changed", "repo:modified"],
            "configuration": {"secret": self.c.secret},
        })
    }

    fn ours(&self, r: &Repo) -> Result<Vec<i64>, Error> {
        let resp = check(self.fetch(&format!("{}?limit=100", Self::path(r)))?)?;
        #[derive(Deserialize, Default)]
        #[serde(default)]
        struct H<'a> {
            #[serde(deserialize_with = "nullable")]
            id: i64,
            #[serde(borrow)]
            url: Text<'a>,
        }
        #[derive(Deserialize, Default)]
        #[serde(default)]
        struct Page<'a> {
            #[serde(borrow, deserialize_with = "nullable")]
            values: Vec<H<'a>>,
        }
        let page: Page<'_> = resp.decode()?;
        Ok(page
            .values
            .iter()
            .filter(|x| x.url.as_str() == self.c.hook_url)
            .map(|x| x.id)
            .collect())
    }
}

impl Source for BitbucketServer {
    fn kind(&self) -> &'static str {
        "bitbucket-server"
    }

    fn whoami(&self) -> Result<Identity, Error> {
        let r = self
            .http
            .get(&format!("{}/plugins/servlet/applinks/whoami", self.c.url))?;
        if r.status == 401 {
            return Err(rejected());
        }
        let r = check(r)?;
        Ok(Identity {
            login: String::from_utf8_lossy(&r.body).trim().to_string(),
            ..Default::default()
        })
    }

    fn list(&self, owner: &str) -> Result<Vec<Arc<Repo>>, Error> {
        let mut out = Vec::new();
        let mut start: i64 = 0;
        loop {
            let path = format!(
                "/projects/{}/repos?limit=100&start={start}",
                path_escape(owner)
            );
            let r = self.fetch(&path)?;
            if !r.ok() {
                return Err(fail(&r, &format!("listing {owner}")));
            }
            #[derive(Deserialize, Default)]
            #[serde(default)]
            struct Page<'a> {
                #[serde(borrow, deserialize_with = "nullable")]
                values: Vec<BsRepo<'a>>,
                #[serde(rename = "isLastPage", deserialize_with = "nullable")]
                is_last_page: bool,
                #[serde(rename = "nextPageStart", deserialize_with = "nullable")]
                next_page_start: i64,
            }
            let (rows, next) = self.pages.decode_meta(&path, &r.body, |b| {
                tolerant(non_empty(b)?, |b| {
                    let page: Page<'_> = serde_json::from_slice(b)?;
                    let next = if page.is_last_page || page.next_page_start <= start {
                        String::new()
                    } else {
                        page.next_page_start.to_string()
                    };
                    let rows = page.values.into_iter().map(BsRepo::into_repo).collect();
                    Ok((rows, next))
                })
            })?;
            out.extend(rows.iter().cloned());
            match next.parse() {
                Ok(n) => start = n,
                Err(_) => return Ok(out),
            }
        }
    }

    fn get(&self, owner: &str, name: &str) -> Result<Option<Arc<Repo>>, Error> {
        let r = self.fetch(&format!(
            "/projects/{}/repos/{}",
            path_escape(owner),
            path_escape(name)
        ))?;
        if r.status == 404 {
            return Ok(None);
        }
        let r = check(r)?;
        let repo = decode_with(&r, |b| {
            Ok(serde_json::from_slice::<BsRepo<'_>>(b)?.into_repo())
        })?;
        Ok(Some(Arc::new(repo)))
    }

    fn owner_exists(&self, owner: &str) -> Result<bool, Error> {
        Ok(self
            .fetch(&format!("/projects/{}", path_escape(owner)))?
            .ok())
    }

    fn hooks(&self) -> Option<&dyn Hooks> {
        Some(self)
    }
}

impl Provider for BitbucketServer {
    fn clone_url(&self, r: &Repo) -> String {
        format!(
            "{}/scm/{}/{}.git",
            self.c.url,
            r.owner.to_lowercase(),
            r.name
        )
    }

    fn git_header(&self) -> Result<String, Error> {
        Ok(format!("Authorization: Bearer {}", self.c.token))
    }

    fn parse_webhook(
        &self,
        h: &Headers,
        body: &[u8],
        secret: &str,
    ) -> Result<Option<Event>, Error> {
        if !valid_mac(secret, body, h.get("X-Hub-Signature"), "sha256=") {
            return Err(Error::Signature);
        }
        if !h.get("X-Event-Key").starts_with("repo:") {
            return Ok(None);
        }
        #[derive(Deserialize, Default)]
        #[serde(default)]
        struct R<'a> {
            #[serde(deserialize_with = "nullable")]
            id: i64,
            #[serde(borrow)]
            slug: Text<'a>,
            #[serde(borrow)]
            project: Option<ProjectKey<'a>>,
        }
        #[derive(Deserialize, Default)]
        #[serde(default)]
        struct P<'a> {
            #[serde(borrow)]
            repository: Option<R<'a>>,
        }
        tolerant(body, |b| {
            let p: P<'_> = serde_json::from_slice(b)?;
            let repo = p.repository.unwrap_or_default();
            if repo.slug.is_empty() {
                return Ok(None);
            }
            Ok(Some(Event {
                kind: Change::Changed,
                owner: repo
                    .project
                    .map(|p| p.key.into_string())
                    .unwrap_or_default(),
                name: repo.slug.into_string(),
                id: repo.id,
            }))
        })
    }
}

impl Hooks for BitbucketServer {
    fn global(&self, _known: i64, _force: bool) -> Result<(i64, String), Error> {
        Err(Error::other(NO_GLOBAL_HOOKS))
    }

    fn repo(&self, r: &Repo, force: bool) -> Result<String, Error> {
        let mine = self.ours(r)?;
        if !mine.is_empty() && !force {
            return Ok("exists".into());
        }
        let path = Self::path(r);
        let (method, target, result) = match mine.first() {
            Some(id) => ("PUT", format!("{}{path}/{id}", self.base), "updated"),
            None => ("POST", format!("{}{path}", self.base), "created"),
        };
        check(self.http.json(method, &target, &self.hook_body())?)?;
        Ok(result.into())
    }

    fn remove_global(&self, _id: i64) -> bool {
        false
    }

    fn remove_repo(&self, r: &Repo) -> i64 {
        let mine = match self.ours(r) {
            Ok(m) => m,
            Err(e) => {
                crate::warnf!("{}: listing webhooks: {e}", r.full());
                return 0;
            }
        };
        let path = Self::path(r);
        mine.iter()
            .filter(|id| warn_removal(r, &self.http, &format!("{}{path}/{id}", self.base)))
            .count() as i64
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Mutex, OnceLock};

    use super::*;
    use crate::forge::common::fake::{self, TOKEN, mac, query};
    use crate::httpx::base64;
    use crate::testutil::{Reply, TestServer};

    fn bb_repo(uuid: &str, slug: &str, private: bool, main: Value) -> Value {
        json!({"uuid": uuid, "slug": slug, "full_name": format!("ws/{slug}"), "is_private": private,
            "description": "d", "website": "https://x.example", "updated_on": "2026-09-01T10:00:00Z",
            "mainbranch": main, "parent": null})
    }

    #[test]
    fn cloud() {
        let url: Arc<OnceLock<String>> = Arc::new(OnceLock::new());
        let hooks: Arc<Mutex<Vec<Value>>> = Arc::new(Mutex::new(Vec::new()));
        let (u, hk) = (url.clone(), hooks.clone());
        let srv = TestServer::start(move |r| {
            let m = r.method.as_str();
            let p = r.path();
            match p {
                "/2.0/user" => Reply::status(403).body("no"),
                "/2.0/repositories" if query(&r.target, "role") == "member" => {
                    fake::js(&json!({"values": []}))
                }
                "/2.0/repositories/ws" if query(&r.target, "page").is_empty() => fake::js(&json!({
                    "values": [bb_repo("{u1}", "a", true, json!({"name": "main"}))],
                    "next": format!("{}/2.0/repositories/ws?page=2", u.get().unwrap())
                })),
                "/2.0/repositories/ws" => {
                    fake::js(&json!({"values": [bb_repo("{u2}", "b", false, Value::Null)]}))
                }
                "/2.0/repositories/ws/a" => {
                    fake::js(&bb_repo("{u1}", "a", true, json!({"name": "main"})))
                }
                "/2.0/workspaces/ws" => fake::js(&json!({"slug": "ws"})),
                "/2.0/repositories/ws/a/hooks" if m == "GET" => {
                    fake::js(&json!({"values": *hk.lock().unwrap()}))
                }
                "/2.0/repositories/ws/a/hooks" if m == "POST" => {
                    let mut h: Value = serde_json::from_slice(&r.body).unwrap();
                    h["uuid"] = json!("{h1}");
                    hk.lock().unwrap().push(h.clone());
                    fake::created(&h)
                }
                _ if m == "DELETE" && p.starts_with("/2.0/repositories/ws/a/hooks/") => {
                    hk.lock().unwrap().clear();
                    Reply::status(204)
                }
                _ => fake::not_found(),
            }
        });
        url.set(srv.url()).unwrap();
        let cfg = fake::config("bitbucket-cloud", &srv.url());
        let mut b = BitbucketCloud::new(&cfg);
        b.http.set_sleep(|_| {});
        assert_eq!(b.whoami().unwrap().login, "token");
        let repos = b.list("ws").unwrap();
        assert_eq!(repos.len(), 2);
        assert!(repos[0].private && !repos[0].empty);
        assert_eq!(repos[0].default_branch, "main");
        assert_eq!(repos[0].website, "https://x.example");
        assert!(!repos[1].private && repos[1].empty);
        assert_ne!(repos[0].id, repos[1].id);
        assert!(
            srv.requests()
                .iter()
                .all(|s| s.authorization == format!("Bearer {TOKEN}"))
        );
        assert_eq!(Source::get(&b, "ws", "a").unwrap().unwrap().name, "a");
        assert!(Source::get(&b, "ws", "gone").unwrap().is_none());
        assert!(b.owner_exists("ws").unwrap());
        assert!(!b.owner_exists("nobody").unwrap());
        assert_eq!(
            b.git_header().unwrap(),
            format!(
                "Authorization: Basic {}",
                base64(format!("x-token-auth:{TOKEN}").as_bytes())
            )
        );

        let h = b.hooks().unwrap();
        let repo = Repo {
            owner: "ws".into(),
            name: "a".into(),
            ..Default::default()
        };
        assert_eq!(h.repo(&repo, false).unwrap(), "created");
        assert_eq!(hooks.lock().unwrap()[0]["url"], cfg.hook_url().as_str());
        assert_eq!(h.repo(&repo, false).unwrap(), "exists");
        assert_eq!(h.remove_repo(&repo), 1);
        assert!(
            srv.lines()
                .contains(&"DELETE /2.0/repositories/ws/a/hooks/%7Bh1%7D".to_string())
        );

        let body = r#"{"repository":{"uuid":"{u1}","full_name":"ws/a"}}"#;
        let sig = format!("sha256={}", mac("s3cret", body));
        let hdr = fake::headers(&[("X-Hub-Signature", &sig), ("X-Event-Key", "repo:push")]);
        fake::must_event(
            b.parse_webhook(&hdr, body.as_bytes(), "s3cret"),
            Change::Changed,
            "ws",
            "a",
            hash_id("{u1}"),
        );
        let unsigned = fake::headers(&[("X-Event-Key", "repo:push")]);
        assert_eq!(
            b.parse_webhook(&unsigned, body.as_bytes(), "s3cret"),
            Err(Error::Signature)
        );
        let other = fake::headers(&[
            ("X-Hub-Signature", &sig),
            ("X-Event-Key", "pullrequest:created"),
        ]);
        assert!(
            b.parse_webhook(&other, body.as_bytes(), "s3cret")
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn cloud_never_follows_next_to_another_host() {
        let srv = TestServer::start(|_| {
            fake::js(&json!({
                "values": [bb_repo("{u1}", "a", true, Value::Null)],
                "next": "https://evil.example/2.0/repositories/ws?page=2"
            }))
        });
        let b = BitbucketCloud::new(&fake::config("bitbucket-cloud", &srv.url()));
        let err = b.list("ws").unwrap_err();
        assert!(err.to_string().contains("refusing"), "{err}");
        assert_eq!(srv.requests().len(), 1);
    }

    #[test]
    fn listings_reuse_unchanged_pages() {
        let url: Arc<OnceLock<String>> = Arc::new(OnceLock::new());
        let u = url.clone();
        let srv = TestServer::start(move |r| match r.path() {
            "/2.0/repositories/ws" if query(&r.target, "page").is_empty() => fake::js(&json!({
                "values": [bb_repo("{u1}", "a", true, Value::Null)],
                "next": format!("{}/2.0/repositories/ws?page=2", u.get().unwrap())
            })),
            "/2.0/repositories/ws" => {
                fake::js(&json!({"values": [bb_repo("{u2}", "b", true, Value::Null)]}))
            }
            _ if query(&r.target, "start") == "0" => fake::js(
                &json!({"values": [bs_repo(1, "a", false)], "isLastPage": false, "nextPageStart": 1}),
            ),
            _ => fake::js(&json!({"values": [bs_repo(2, "b", true)], "isLastPage": true})),
        });
        url.set(srv.url()).unwrap();
        let cloud = BitbucketCloud::new(&fake::config("bitbucket-cloud", &srv.url()));
        let a = cloud.list("ws").unwrap();
        let b = cloud.list("ws").unwrap();
        assert_eq!(b.len(), 2);
        assert!(Arc::ptr_eq(&a[0], &b[0]) && Arc::ptr_eq(&a[1], &b[1]));
        let server = BitbucketServer::new(&fake::config("bitbucket-server", &srv.url()));
        let a = server.list("PROJ").unwrap();
        let b = server.list("PROJ").unwrap();
        assert_eq!(b.len(), 2);
        assert!(Arc::ptr_eq(&a[0], &b[0]) && Arc::ptr_eq(&a[1], &b[1]));
        assert_eq!(srv.requests().len(), 8);
    }

    #[test]
    fn cloud_rejected_token() {
        let srv = TestServer::start(|_| Reply::status(401));
        let mut b = BitbucketCloud::new(&fake::config("bitbucket-cloud", &srv.url()));
        b.http.set_sleep(|_| {});
        let err = b.whoami().unwrap_err();
        assert_eq!(err.status(), Some(401));
        assert!(err.to_string().contains("rejected"));
    }

    #[test]
    fn hash_ids_match_go() {
        assert_eq!(hash_id(""), (0xcbf2_9ce4_8422_2325u64 >> 1) as i64);
        assert_eq!(hash_id("a"), (0xaf63_dc4c_8601_ec8cu64 >> 1) as i64);
        assert!(hash_id("{u1}") >= 0);
    }

    fn bs_repo(id: i64, slug: &str, public: bool) -> Value {
        json!({"id": id, "slug": slug, "public": public, "archived": id == 2, "description": "d",
            "project": {"key": "PROJ"}})
    }

    #[test]
    fn server() {
        let hooks: Arc<Mutex<Vec<Value>>> = Arc::new(Mutex::new(Vec::new()));
        let hk = hooks.clone();
        let srv = TestServer::start(move |r| {
            let m = r.method.as_str();
            match r.path() {
                "/plugins/servlet/applinks/whoami" => Reply::status(200).body("alice\n"),
                "/rest/api/1.0/projects/PROJ" => fake::js(&json!({"key": "PROJ"})),
                "/rest/api/1.0/projects/PROJ/repos" if query(&r.target, "start") == "0" => {
                    fake::js(
                        &json!({"values": [bs_repo(1, "a", false)], "isLastPage": false, "nextPageStart": 1}),
                    )
                }
                "/rest/api/1.0/projects/PROJ/repos" => {
                    fake::js(&json!({"values": [bs_repo(2, "b", true)], "isLastPage": true}))
                }
                "/rest/api/1.0/projects/PROJ/repos/a" => fake::js(&bs_repo(1, "a", false)),
                "/rest/api/1.0/projects/PROJ/repos/a/webhooks" if m == "GET" => {
                    fake::js(&json!({"values": *hk.lock().unwrap()}))
                }
                "/rest/api/1.0/projects/PROJ/repos/a/webhooks" if m == "POST" => {
                    let mut h: Value = serde_json::from_slice(&r.body).unwrap();
                    h["id"] = json!(5);
                    hk.lock().unwrap().push(h.clone());
                    fake::created(&h)
                }
                "/rest/api/1.0/projects/PROJ/repos/a/webhooks/5" if m == "DELETE" => {
                    hk.lock().unwrap().clear();
                    Reply::status(204)
                }
                _ => fake::not_found(),
            }
        });
        let cfg = fake::config("bitbucket-server", &srv.url());
        let mut b = BitbucketServer::new(&cfg);
        b.http.set_sleep(|_| {});
        assert_eq!(b.whoami().unwrap().login, "alice");
        let repos = b.list("PROJ").unwrap();
        assert_eq!(repos.len(), 2);
        assert!(repos[0].private && !repos[1].private && repos[1].archived);
        assert_eq!(repos[0].owner, "PROJ");
        assert!(
            srv.requests()
                .iter()
                .all(|s| s.authorization == format!("Bearer {TOKEN}"))
        );
        assert!(Source::get(&b, "PROJ", "gone").unwrap().is_none());
        assert_eq!(Source::get(&b, "PROJ", "a").unwrap().unwrap().id, 1);
        assert!(b.owner_exists("PROJ").unwrap());
        assert!(b.clone_url(&repos[0]).ends_with("/scm/proj/a.git"));
        assert_eq!(
            b.git_header().unwrap(),
            format!("Authorization: Bearer {TOKEN}")
        );
        let h = b.hooks().unwrap();
        let repo = Repo {
            owner: "PROJ".into(),
            name: "a".into(),
            ..Default::default()
        };
        assert_eq!(h.repo(&repo, false).unwrap(), "created");
        assert_eq!(
            hooks.lock().unwrap()[0]["configuration"]["secret"],
            fake::SECRET
        );
        assert_eq!(h.repo(&repo, false).unwrap(), "exists");
        assert_eq!(h.remove_repo(&repo), 1);

        let body = r#"{"repository":{"id":1,"slug":"a","project":{"key":"PROJ"}}}"#;
        let sig = format!("sha256={}", mac("s3cret", body));
        let hdr = fake::headers(&[
            ("X-Hub-Signature", &sig),
            ("X-Event-Key", "repo:refs_changed"),
        ]);
        fake::must_event(
            b.parse_webhook(&hdr, body.as_bytes(), "s3cret"),
            Change::Changed,
            "PROJ",
            "a",
            1,
        );
    }

    #[test]
    fn server_paging_stops_when_the_cursor_does_not_move() {
        let srv = TestServer::start(|_| {
            fake::js(&json!({"values": [bs_repo(1, "a", false)], "isLastPage": false}))
        });
        let b = BitbucketServer::new(&fake::config("bitbucket-server", &srv.url()));
        assert_eq!(b.list("PROJ").unwrap().len(), 1);
        assert_eq!(srv.requests().len(), 1);
    }
}
