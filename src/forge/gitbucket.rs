use std::sync::{Arc, Mutex};

use serde::Deserialize;
use serde_json::{Value, json};

use super::common::{
    Common, Text, check, cut, decode_with, deleted, eq_fold, nullable, path_escape, rejected,
    tolerant,
};
use super::gitea::our_hooks;
use super::{Change, Event, Hooks, Identity, PageCache, Provider, Repo, Source, valid_mac};
use crate::config::Config;
use crate::error::Error;
use crate::httpx::{Client, Headers, Response, fail};

pub(crate) const NO_GLOBAL_HOOKS: &str = "this server has no system-wide webhooks";

pub struct GitBucket {
    c: Common,
    base: String,
    http: Client,
    pages: PageCache,
    login: Mutex<String>,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct Owner<'a> {
    #[serde(borrow)]
    login: Text<'a>,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct GbRepo<'a> {
    #[serde(deserialize_with = "nullable")]
    id: i64,
    #[serde(borrow)]
    name: Text<'a>,
    #[serde(borrow)]
    full_name: Text<'a>,
    #[serde(deserialize_with = "nullable")]
    private: bool,
    #[serde(borrow)]
    description: Text<'a>,
    #[serde(borrow)]
    homepage: Text<'a>,
    #[serde(borrow)]
    default_branch: Text<'a>,
    #[serde(deserialize_with = "nullable")]
    fork: bool,
    #[serde(deserialize_with = "nullable")]
    archived: bool,
    #[serde(borrow)]
    updated_at: Option<Text<'a>>,
    #[serde(borrow)]
    pushed_at: Option<Text<'a>>,
    #[serde(borrow)]
    owner: Option<Owner<'a>>,
}

impl GbRepo<'_> {
    fn into_repo(self) -> Repo {
        let owner = match &self.owner {
            Some(o) if !o.login.is_empty() => o.login.as_str(),
            _ => cut(self.full_name.as_str(), '/').0,
        }
        .to_string();
        let updated = self
            .pushed_at
            .or(self.updated_at)
            .map(Text::into_string)
            .unwrap_or_default();
        Repo {
            id: self.id,
            owner,
            name: self.name.into_string(),
            private: self.private,
            description: self.description.trimmed(),
            website: self.homepage.trimmed(),
            archived: self.archived,
            default_branch: self.default_branch.into_string(),
            fork: self.fork,
            updated,
            ..Default::default()
        }
    }
}

fn decode_repos(b: &[u8]) -> Result<Vec<Repo>, Error> {
    let rows: Option<Vec<GbRepo<'_>>> = serde_json::from_slice(b)?;
    Ok(rows
        .unwrap_or_default()
        .into_iter()
        .map(GbRepo::into_repo)
        .collect())
}

impl GitBucket {
    pub fn new(cfg: &Config) -> GitBucket {
        let c = Common::new(cfg);
        GitBucket {
            base: format!("{}/api/v3", c.url),
            http: c.client("token"),
            pages: PageCache::default(),
            login: Mutex::new(String::new()),
            c,
        }
    }

    fn fetch(&self, path: &str) -> Result<Response, Error> {
        self.http.get(&format!("{}{path}", self.base))
    }

    fn path(r: &Repo) -> String {
        format!(
            "/repos/{}/{}/hooks",
            path_escape(&r.owner),
            path_escape(&r.name)
        )
    }

    fn hook_body(&self) -> Value {
        json!({
            "name": "web",
            "active": true,
            "events": ["push", "create", "delete"],
            "config": {
                "url": self.c.hook_url,
                "content_type": "json",
                "secret": self.c.secret,
            },
        })
    }

    fn ours(&self, path: &str) -> Result<Vec<i64>, Error> {
        let r = self.fetch(path)?;
        if !r.ok() {
            return Err(fail(&r, path));
        }
        our_hooks(&r, &self.c.hook_url)
    }
}

impl Source for GitBucket {
    fn kind(&self) -> &'static str {
        "gitbucket"
    }

    fn whoami(&self) -> Result<Identity, Error> {
        let r = self.fetch("/user")?;
        if r.status == 401 {
            return Err(rejected());
        }
        let r = check(r)?;
        let u: Owner<'_> = r.decode()?;
        let login = u.login.into_string();
        *self.login.lock().unwrap_or_else(|e| e.into_inner()) = login.clone();
        Ok(Identity {
            login,
            ..Default::default()
        })
    }

    fn list(&self, owner: &str) -> Result<Vec<Arc<Repo>>, Error> {
        let r = self.fetch(&format!("/users/{}/repos", path_escape(owner)))?;
        if !r.ok() {
            return Err(fail(&r, &format!("listing {owner}")));
        }
        let page = self.pages.decode(&format!("list/{owner}"), &r.body, |b| {
            tolerant(b, decode_repos)
        })?;
        Ok(page
            .iter()
            .filter(|x| eq_fold(&x.owner, owner))
            .cloned()
            .collect())
    }

    fn get(&self, owner: &str, name: &str) -> Result<Option<Arc<Repo>>, Error> {
        let r = self.fetch(&format!(
            "/repos/{}/{}",
            path_escape(owner),
            path_escape(name)
        ))?;
        if r.status == 404 {
            return Ok(None);
        }
        let r = check(r)?;
        let repo = decode_with(&r, |b| {
            Ok(serde_json::from_slice::<GbRepo<'_>>(b)?.into_repo())
        })?;
        Ok(Some(Arc::new(repo)))
    }

    fn owner_exists(&self, owner: &str) -> Result<bool, Error> {
        Ok(self.fetch(&format!("/users/{}", path_escape(owner)))?.ok())
    }

    fn hooks(&self) -> Option<&dyn Hooks> {
        Some(self)
    }
}

impl Provider for GitBucket {
    fn clone_url(&self, r: &Repo) -> String {
        format!("{}/git/{}/{}.git", self.c.url, r.owner, r.name)
    }

    fn git_header(&self) -> Result<String, Error> {
        let mut login = self.login.lock().unwrap_or_else(|e| e.into_inner()).clone();
        if login.is_empty() {
            login = self.whoami()?.login;
        }
        Ok(self.c.basic_header(&login))
    }

    fn parse_webhook(
        &self,
        h: &Headers,
        body: &[u8],
        secret: &str,
    ) -> Result<Option<Event>, Error> {
        if !valid_mac(secret, body, h.get("X-Hub-Signature-256"), "sha256=") {
            return Err(Error::Signature);
        }
        if !matches!(
            h.get("X-Github-Event"),
            "push" | "create" | "delete" | "repository"
        ) {
            return Ok(None);
        }
        #[derive(Deserialize, Default)]
        #[serde(default)]
        struct R<'a> {
            #[serde(deserialize_with = "nullable")]
            id: i64,
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
                id: repo.id,
            }))
        })
    }
}

impl Hooks for GitBucket {
    fn global(&self, _known: i64, _force: bool) -> Result<(i64, String), Error> {
        Err(Error::other(NO_GLOBAL_HOOKS))
    }

    fn repo(&self, r: &Repo, force: bool) -> Result<String, Error> {
        let path = Self::path(r);
        let mine = self.ours(&path)?;
        if !mine.is_empty() && !force {
            return Ok("exists".into());
        }
        let (method, target, result) = match mine.first() {
            Some(id) => ("PATCH", format!("{}{path}/{id}", self.base), "updated"),
            None => ("POST", format!("{}{path}", self.base), "created"),
        };
        check(self.http.json(method, &target, &self.hook_body())?)?;
        Ok(result.into())
    }

    fn remove_global(&self, _id: i64) -> bool {
        false
    }

    fn remove_repo(&self, r: &Repo) -> i64 {
        let path = Self::path(r);
        let Ok(mine) = self.ours(&path) else {
            return 0;
        };
        mine.iter()
            .filter(|id| deleted(&self.http, &format!("{}{path}/{id}", self.base)))
            .count() as i64
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::forge::common::fake::{self, TOKEN, mac};
    use crate::httpx::base64;
    use crate::testutil::{Reply, TestServer};

    #[test]
    fn gitbucket() {
        let hooks: Arc<Mutex<Vec<Value>>> = Arc::new(Mutex::new(Vec::new()));
        let hk = hooks.clone();
        let srv = TestServer::start(move |r| {
            let m = r.method.as_str();
            match r.path() {
                "/api/v3/user" => fake::js(&json!({"login": "root", "site_admin": true})),
                "/api/v3/users/root/repos" => fake::js(&json!([
                    {"id": 1, "name": "r1", "full_name": "root/r1", "private": true, "description": "hola",
                     "default_branch": "main", "fork": false, "archived": null, "updated_at": null,
                     "homepage": null, "owner": {"login": "root"}},
                    {"id": 2, "name": "x", "full_name": "other/x", "private": false,
                     "owner": {"login": "other"}}
                ])),
                "/api/v3/users/root" => fake::js(&json!({"login": "root"})),
                "/api/v3/repos/root/r1" => fake::js(
                    &json!({"id": 1, "name": "r1", "full_name": "root/r1", "private": false, "owner": {"login": "root"}}),
                ),
                "/api/v3/repos/root/r1/hooks" if m == "GET" => {
                    fake::js(&json!(*hk.lock().unwrap()))
                }
                "/api/v3/repos/root/r1/hooks" if m == "POST" => {
                    let mut h: Value = serde_json::from_slice(&r.body).unwrap();
                    h["id"] = json!(7);
                    hk.lock().unwrap().push(h.clone());
                    fake::js(&h)
                }
                "/api/v3/repos/root/r1/hooks/7" if m == "DELETE" => {
                    hk.lock().unwrap().clear();
                    Reply::status(204)
                }
                _ => fake::not_found(),
            }
        });
        let cfg = fake::config("gitbucket", &srv.url());
        let mut g = GitBucket::new(&cfg);
        g.http.set_sleep(|_| {});
        let repos = g.list("root").unwrap();
        assert_eq!(repos.len(), 1);
        let r = &repos[0];
        assert!(r.private);
        assert_eq!(r.description, "hola");
        assert_eq!(r.updated, "");
        assert_eq!(r.website, "");
        assert_eq!(srv.requests()[0].authorization, format!("token {TOKEN}"));
        assert!(!Source::get(&g, "root", "r1").unwrap().unwrap().private);
        assert!(Source::get(&g, "root", "gone").unwrap().is_none());
        assert!(g.clone_url(r).ends_with("/git/root/r1.git"));
        assert_eq!(
            g.git_header().unwrap(),
            format!(
                "Authorization: Basic {}",
                base64(format!("root:{TOKEN}").as_bytes())
            )
        );
        assert!(g.owner_exists("root").unwrap());

        let h = g.hooks().unwrap();
        assert_eq!(h.global(0, false).unwrap_err().to_string(), NO_GLOBAL_HOOKS);
        assert!(!h.remove_global(1));
        let repo = Repo {
            owner: "root".into(),
            name: "r1".into(),
            ..Default::default()
        };
        assert_eq!(h.repo(&repo, false).unwrap(), "created");
        assert_eq!(
            hooks.lock().unwrap()[0]["config"]["url"],
            cfg.hook_url().as_str()
        );
        assert_eq!(h.repo(&repo, false).unwrap(), "exists");
        assert_eq!(h.remove_repo(&repo), 1);
    }

    #[test]
    fn pushed_at_wins_over_updated_at() {
        let rows = br#"[{"id":1,"name":"a","full_name":"o/a","updated_at":"u","pushed_at":"p"},
            {"id":2,"name":"b","full_name":"o/b","updated_at":"u"}]"#;
        let repos = decode_repos(rows).unwrap();
        assert_eq!(repos[0].updated, "p");
        assert_eq!(repos[1].updated, "u");
        assert_eq!(repos[1].owner, "o");
    }

    #[test]
    fn webhooks() {
        let srv = TestServer::start(|_| Reply::status(200));
        let g = GitBucket::new(&fake::config("gitbucket", &srv.url()));
        let body = r#"{"ref":"refs/heads/main","repository":{"id":1,"full_name":"root/r1"}}"#;
        let sig = format!("sha256={}", mac("s3cret", body));
        let h = fake::headers(&[("X-Hub-Signature-256", &sig), ("X-Github-Event", "push")]);
        fake::must_event(
            g.parse_webhook(&h, body.as_bytes(), "s3cret"),
            Change::Changed,
            "root",
            "r1",
            1,
        );
        let unsigned = fake::headers(&[("X-Github-Event", "push")]);
        assert_eq!(
            g.parse_webhook(&unsigned, body.as_bytes(), "s3cret"),
            Err(Error::Signature)
        );
        let h = fake::headers(&[("X-Hub-Signature-256", &sig), ("X-Github-Event", "issues")]);
        assert!(
            g.parse_webhook(&h, body.as_bytes(), "s3cret")
                .unwrap()
                .is_none()
        );
    }
}
