use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use serde::Deserialize;
use serde_json::{Value, json};

use super::common::{
    Common, Text, check, cut, decode_with, deleted, eq_fold, nullable, path_escape, rejected,
    tolerant,
};
use super::{
    Change, Event, Hooks, Identity, PageCache, Provider, ROLE_ADMIN, ROLE_READ, ROLE_WRITE, Repo,
    Source, valid_mac,
};
use crate::config::Config;
use crate::error::Error;
use crate::httpx::{Client, Headers, Response, fail};

const GITEA_HOOK_EVENTS: &[&str] = &["push", "create", "delete", "repository"];
const GOGS_HOOK_EVENTS: &[&str] = &["push", "create", "delete"];
const PAGE: usize = 50;

pub struct Gitea {
    c: Common,
    kind: &'static str,
    base: String,
    http: Client,
    pages: PageCache,
    ids: Mutex<HashMap<String, i64>>,
    bases: Mutex<HashMap<String, String>>,
    login: Mutex<String>,
    orphan: AtomicBool,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct Owner<'a> {
    #[serde(borrow)]
    login: Text<'a>,
    #[serde(borrow)]
    username: Text<'a>,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct Perms {
    #[serde(deserialize_with = "nullable")]
    admin: bool,
    #[serde(deserialize_with = "nullable")]
    push: bool,
}

#[derive(Deserialize, Default)]
#[serde(default)]
pub(crate) struct GiteaRepo<'a> {
    #[serde(deserialize_with = "nullable")]
    id: i64,
    #[serde(borrow)]
    name: Text<'a>,
    #[serde(borrow)]
    full_name: Text<'a>,
    #[serde(borrow)]
    owner: Option<Owner<'a>>,
    #[serde(deserialize_with = "nullable")]
    private: bool,
    #[serde(borrow)]
    description: Text<'a>,
    #[serde(borrow)]
    website: Text<'a>,
    #[serde(deserialize_with = "nullable")]
    archived: bool,
    #[serde(borrow)]
    default_branch: Text<'a>,
    #[serde(deserialize_with = "nullable")]
    fork: bool,
    #[serde(deserialize_with = "nullable")]
    mirror: bool,
    #[serde(deserialize_with = "nullable")]
    empty: bool,
    #[serde(deserialize_with = "nullable")]
    topics: Vec<String>,
    #[serde(borrow)]
    updated_at: Text<'a>,
    permissions: Option<Perms>,
}

impl GiteaRepo<'_> {
    fn role(&self) -> &'static str {
        match &self.permissions {
            None => "",
            Some(p) if p.admin => ROLE_ADMIN,
            Some(p) if p.push => ROLE_WRITE,
            Some(_) => ROLE_READ,
        }
    }

    pub(crate) fn into_repo(self) -> Repo {
        let role = self.role();
        let full = self.full_name.as_str();
        let owner = match &self.owner {
            Some(o) if !o.login.is_empty() => o.login.as_str(),
            Some(o) if !o.username.is_empty() => o.username.as_str(),
            _ => cut(full, '/').0,
        }
        .to_string();
        let name = if self.name.is_empty() {
            full[full.rfind('/').map_or(0, |i| i + 1)..].to_string()
        } else {
            self.name.as_str().to_string()
        };
        Repo {
            id: self.id,
            owner,
            name,
            private: self.private,
            description: self.description.trimmed(),
            website: self.website.trimmed(),
            archived: self.archived,
            default_branch: self.default_branch.into_string(),
            fork: self.fork,
            mirror: self.mirror,
            empty: self.empty,
            topics: self.topics,
            updated: self.updated_at.into_string(),
            role,
        }
    }
}

fn decode_search(b: &[u8]) -> Result<Vec<Repo>, Error> {
    #[derive(Deserialize)]
    struct Body<'a> {
        #[serde(borrow, default)]
        data: Option<Vec<GiteaRepo<'a>>>,
    }
    let body: Body<'_> = serde_json::from_slice(b)?;
    Ok(convert(body.data.unwrap_or_default()))
}

fn decode_plain(b: &[u8]) -> Result<Vec<Repo>, Error> {
    let rows: Option<Vec<GiteaRepo<'_>>> = serde_json::from_slice(b)?;
    Ok(convert(rows.unwrap_or_default()))
}

fn more_rows(r: &Response, seen: usize) -> bool {
    r.headers
        .value("X-Total-Count")
        .and_then(|v| v.trim().parse::<usize>().ok())
        .is_some_and(|total| seen < total)
}

fn convert(rows: Vec<GiteaRepo<'_>>) -> Vec<Repo> {
    rows.into_iter().map(GiteaRepo::into_repo).collect()
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct HookConfig<'a> {
    #[serde(borrow)]
    url: Text<'a>,
}

#[derive(Deserialize, Default)]
#[serde(default)]
pub(crate) struct HookInfo<'a> {
    #[serde(deserialize_with = "nullable")]
    pub id: i64,
    #[serde(borrow)]
    config: Option<HookConfig<'a>>,
}

impl HookInfo<'_> {
    pub fn url(&self) -> &str {
        self.config.as_ref().map_or("", |c| c.url.as_str())
    }
}

pub(crate) fn our_hooks(r: &Response, hook_url: &str) -> Result<Vec<i64>, Error> {
    let all: Option<Vec<HookInfo<'_>>> = r.decode()?;
    Ok(all
        .unwrap_or_default()
        .iter()
        .filter(|x| x.url() == hook_url)
        .map(|x| x.id)
        .collect())
}

impl Gitea {
    pub fn new(cfg: &Config) -> Gitea {
        let kind = match cfg.source.kind.as_str() {
            "gogs" => "gogs",
            "gitea" => "gitea",
            _ => "forgejo",
        };
        let c = Common::new(cfg);
        Gitea {
            kind,
            base: format!("{}/api/v1", c.url),
            http: c.client("token"),
            pages: PageCache::default(),
            ids: Mutex::new(HashMap::new()),
            bases: Mutex::new(HashMap::new()),
            login: Mutex::new(String::new()),
            orphan: AtomicBool::new(false),
            c,
        }
    }

    fn fetch(&self, path: &str) -> Result<Response, Error> {
        self.http.get(&format!("{}{path}", self.base))
    }

    fn owner_id(&self, owner: &str) -> Result<i64, Error> {
        if let Some(id) = self
            .ids
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(owner)
        {
            return Ok(*id);
        }
        let r = self.fetch(&format!("/users/{}", path_escape(owner)))?;
        if !r.ok() {
            return Err(Error::api(
                r.status,
                format!("cannot look up source owner '{owner}'"),
            ));
        }
        #[derive(Deserialize)]
        struct U {
            #[serde(default, deserialize_with = "nullable")]
            id: i64,
        }
        let u: U = r.decode()?;
        self.ids
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(owner.to_string(), u.id);
        Ok(u.id)
    }

    fn list_search(&self, owner: &str) -> Result<Vec<Arc<Repo>>, Error> {
        let uid = self.owner_id(owner)?;
        let mut out = Vec::new();
        let mut rows = 0;
        let mut n = 1;
        loop {
            let q = format!("limit={PAGE}&page={n}&uid={uid}");
            let r = self.fetch(&format!("/repos/search?{q}"))?;
            if !r.ok() {
                return Err(fail(&r, &format!("listing {owner}")));
            }
            let page = self.pages.decode(&format!("search?{q}"), &r.body, |b| {
                tolerant(b, decode_search)
            })?;
            out.extend(page.iter().filter(|x| eq_fold(&x.owner, owner)).cloned());
            rows += page.len();
            if page.is_empty() || (page.len() < PAGE && !more_rows(&r, rows)) {
                return Ok(out);
            }
            n += 1;
        }
    }

    fn first_page(&self, owner: &str) -> Result<Option<(String, Response)>, Error> {
        let cached = self
            .bases
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(owner)
            .cloned();
        if let Some(base) = cached {
            let r = self.fetch(&format!("{base}?limit={PAGE}&page=1"))?;
            if r.ok() {
                return Ok(Some((base, r)));
            }
            self.bases
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .remove(owner);
        }
        for cand in [
            format!("/orgs/{owner}/repos"),
            format!("/users/{owner}/repos"),
        ] {
            let r = self.fetch(&format!("{cand}?limit={PAGE}&page=1"))?;
            if r.ok() {
                self.bases
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .insert(owner.to_string(), cand.clone());
                return Ok(Some((cand, r)));
            }
        }
        Ok(None)
    }

    fn list_plain(&self, owner: &str) -> Result<Vec<Arc<Repo>>, Error> {
        let Some((base, first)) = self.first_page(owner)? else {
            return Err(Error::api(404, format!("cannot list repos of '{owner}'")));
        };
        let mut first = Some(first);
        let mut seen = HashSet::new();
        let mut out = Vec::new();
        let mut n = 1;
        loop {
            let q = format!("limit={PAGE}&page={n}");
            let r = match first.take() {
                Some(r) => r,
                None => self.fetch(&format!("{base}?{q}"))?,
            };
            if !r.ok() {
                return Err(fail(&r, &format!("listing {owner}")));
            }
            let page = self.pages.decode(&format!("{base}?{q}"), &r.body, |b| {
                tolerant(b, decode_plain)
            })?;
            let before = out.len();
            for x in page.iter() {
                if seen.insert(x.id) {
                    out.push(x.clone());
                }
            }
            if out.len() == before {
                return Ok(out);
            }
            n += 1;
        }
    }

    fn hook_body(&self) -> Value {
        let (events, typ) = if self.kind == "gogs" {
            (GOGS_HOOK_EVENTS, "gogs")
        } else {
            (GITEA_HOOK_EVENTS, "gitea")
        };
        json!({
            "type": typ,
            "active": true,
            "events": events,
            "config": {
                "url": self.c.hook_url,
                "content_type": "json",
                "secret": self.c.secret,
            },
        })
    }

    fn ours(&self, base: &str) -> Result<Vec<i64>, Error> {
        let r = self.fetch(base)?;
        if !r.ok() {
            return Err(fail(&r, base));
        }
        our_hooks(&r, &self.c.hook_url)
    }

    fn ensure(&self, base: &str, force: bool) -> Result<String, Error> {
        let mine = self.ours(base)?;
        if !mine.is_empty() && !force {
            return Ok("exists".into());
        }
        let (method, target, result) = match mine.first() {
            Some(id) => ("PATCH", format!("{}{base}/{id}", self.base), "updated"),
            None => ("POST", format!("{}{base}", self.base), "created"),
        };
        check(self.http.json(method, &target, &self.hook_body())?)?;
        Ok(result.into())
    }

    fn admin_hook(&self, id: i64) -> String {
        format!("{}/admin/hooks/{id}", self.base)
    }

    fn is_ours(&self, r: &Response) -> bool {
        r.ok()
            && r.decode::<HookInfo<'_>>()
                .is_ok_and(|x| x.url() == self.c.hook_url)
    }

    fn create_global(&self) -> Result<Option<i64>, Error> {
        let r = self.http.json(
            "POST",
            &format!("{}/admin/hooks", self.base),
            &self.hook_body(),
        );
        let r = match r {
            Ok(r) => check(r)?,
            Err(e) => {
                self.orphan.store(true, Ordering::Relaxed);
                return Err(e);
            }
        };
        Ok(r.decode::<HookInfo<'_>>()
            .ok()
            .map(|x| x.id)
            .filter(|id| *id > 0))
    }

    fn drop_older(&self, keep: i64) {
        let low = (keep - 999).max(1);
        for id in (low..keep).rev() {
            let Ok(r) = self.http.get(&self.admin_hook(id)) else {
                continue;
            };
            if self.is_ours(&r) && deleted(&self.http, &self.admin_hook(id)) {
                crate::infof!("removed a duplicate system webhook (#{id})");
            }
        }
    }

    fn repo_hooks(r: &Repo) -> String {
        format!(
            "/repos/{}/{}/hooks",
            path_escape(&r.owner),
            path_escape(&r.name)
        )
    }
}

impl Source for Gitea {
    fn kind(&self) -> &'static str {
        self.kind
    }

    fn whoami(&self) -> Result<Identity, Error> {
        let r = self.fetch("/user")?;
        if r.status == 401 {
            return Err(rejected());
        }
        let r = check(r)?;
        #[derive(Deserialize, Default)]
        #[serde(default)]
        struct U<'a> {
            #[serde(borrow)]
            login: Text<'a>,
            #[serde(borrow)]
            username: Text<'a>,
            #[serde(deserialize_with = "nullable")]
            is_admin: bool,
        }
        let u: U<'_> = r.decode()?;
        let login = if u.login.is_empty() {
            u.username.into_string()
        } else {
            u.login.into_string()
        };
        *self.login.lock().unwrap_or_else(|e| e.into_inner()) = login.clone();
        Ok(Identity {
            login,
            admin: u.is_admin,
            system_hooks: u.is_admin && self.kind != "gitea",
        })
    }

    fn list(&self, owner: &str) -> Result<Vec<Arc<Repo>>, Error> {
        if self.kind == "gogs" {
            self.list_plain(owner)
        } else {
            self.list_search(owner)
        }
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
            Ok(serde_json::from_slice::<GiteaRepo<'_>>(b)?.into_repo())
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

impl Provider for Gitea {
    fn clone_url(&self, r: &Repo) -> String {
        format!("{}/{}/{}.git", self.c.url, r.owner, r.name)
    }

    fn git_header(&self) -> Result<String, Error> {
        if self.kind != "gogs" {
            return Ok(format!("Authorization: token {}", self.c.token));
        }
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
        let signed = [
            "X-Gitea-Signature",
            "X-Forgejo-Signature",
            "X-Gogs-Signature",
        ]
        .iter()
        .any(|name| valid_mac(secret, body, h.get(name), ""));
        if !signed && !valid_mac(secret, body, h.get("X-Hub-Signature-256"), "sha256=") {
            return Err(Error::Signature);
        }
        let event = ["X-Gitea-Event", "X-Forgejo-Event", "X-Gogs-Event"]
            .iter()
            .map(|name| h.get(name))
            .find(|v| !v.is_empty())
            .unwrap_or("");
        if !matches!(event, "push" | "create" | "delete" | "repository") {
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
            action: Text<'a>,
            #[serde(borrow)]
            repository: Option<R<'a>>,
        }
        tolerant(body, |b| {
            let p: P<'_> = serde_json::from_slice(b)?;
            let repo = p.repository.unwrap_or_default();
            let Some((owner, name)) = repo.full_name.as_str().split_once('/') else {
                return Ok(None);
            };
            let kind = if event == "repository" && p.action.as_str() == "deleted" {
                Change::Removed
            } else {
                Change::Changed
            };
            Ok(Some(Event {
                kind,
                owner: owner.to_string(),
                name: name.to_string(),
                id: repo.id,
            }))
        })
    }
}

impl Hooks for Gitea {
    fn global(&self, known: i64, force: bool) -> Result<(i64, String), Error> {
        if known != 0 && !self.orphan.load(Ordering::Relaxed) {
            let r = self.http.get(&self.admin_hook(known))?;
            if !r.ok() && r.status != 404 {
                return Err(fail(&r, ""));
            }
            if self.is_ours(&r) {
                if !force {
                    return Ok((known, "exists".into()));
                }
                check(
                    self.http
                        .json("PATCH", &self.admin_hook(known), &self.hook_body())?,
                )?;
                return Ok((known, "updated".into()));
            }
        }
        let id = match self.create_global()? {
            Some(id) => id,
            None => {
                self.orphan.store(true, Ordering::Relaxed);
                self.create_global()?
                    .ok_or_else(|| Error::other("cannot read the id of the new system webhook"))?
            }
        };
        self.orphan.store(false, Ordering::Relaxed);
        self.drop_older(id);
        Ok((id, "created".into()))
    }

    fn repo(&self, r: &Repo, force: bool) -> Result<String, Error> {
        self.ensure(&Self::repo_hooks(r), force)
    }

    fn remove_global(&self, id: i64) -> bool {
        id != 0 && deleted(&self.http, &self.admin_hook(id))
    }

    fn remove_repo(&self, r: &Repo) -> i64 {
        let base = Self::repo_hooks(r);
        let Ok(mine) = self.ours(&base) else {
            return 0;
        };
        mine.iter()
            .filter(|id| deleted(&self.http, &format!("{}{base}/{id}", self.base)))
            .count() as i64
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::forge::common::fake::{self, SECRET, TOKEN, mac, query};
    use crate::httpx::base64;
    use crate::testutil::{Reply, TestServer};
    use crate::webhook::Request;

    #[derive(Default)]
    struct State {
        repos: Vec<Value>,
        hooks: Vec<Value>,
        repo_hooks: Vec<Value>,
        plain: bool,
    }

    fn atoi(s: &str, def: usize) -> usize {
        s.parse().ok().filter(|n| *n != 0).unwrap_or(def)
    }

    fn route(st: &Mutex<State>, r: &Request) -> Reply {
        let mut g = st.lock().unwrap();
        let p = r.path();
        let m = r.method.as_str();
        let id_of = |prefix: &str| p.strip_prefix(prefix).unwrap_or("").to_string();
        let same = |h: &Value, id: &str| h["id"].as_i64() == id.parse().ok();
        match p {
            "/api/v1/user" => fake::js(&json!({"login": "alice", "is_admin": true})),
            "/api/v1/users/alice" => fake::js(&json!({"id": 1})),
            "/api/v1/users/acme" => fake::js(&json!({"id": 2})),
            "/api/v1/repos/search" if !g.plain => {
                let page = atoi(query(&r.target, "page"), 1);
                let limit = atoi(query(&r.target, "limit"), 30);
                let uid = query(&r.target, "uid");
                let mine: Vec<&Value> = g
                    .repos
                    .iter()
                    .filter(|d| {
                        let o = d["owner"]["login"].as_str().unwrap();
                        (uid == "1" && (o == "alice" || o == "stale"))
                            || (uid == "2" && o == "acme")
                    })
                    .collect();
                let lo = ((page - 1) * limit).min(mine.len());
                let hi = (page * limit).min(mine.len());
                fake::js(&json!({"ok": true, "data": mine[lo..hi]}))
            }
            "/api/v1/users/alice/repos" if g.plain => {
                if atoi(query(&r.target, "page"), 1) > 1 {
                    return fake::js(&json!([]));
                }
                fake::js(&json!(g.repos))
            }
            "/api/v1/repos/alice/site" => fake::js(&g.repos[0]),
            "/api/v1/admin/hooks" if m == "GET" => fake::js(&json!([])),
            "/api/v1/admin/hooks" if m == "POST" => {
                let mut body: Value = serde_json::from_slice(&r.body).unwrap();
                body["id"] = json!(g.hooks.len() + 100);
                g.hooks.push(body.clone());
                fake::created(&body)
            }
            _ if p.starts_with("/api/v1/admin/hooks/") => {
                let id = id_of("/api/v1/admin/hooks/");
                match m {
                    "GET" => match g.hooks.iter().find(|h| same(h, &id)) {
                        Some(h) => fake::js(h),
                        None => fake::not_found(),
                    },
                    "PATCH" => fake::js(&json!({})),
                    "DELETE" => {
                        g.hooks.retain(|h| !same(h, &id));
                        Reply::status(204)
                    }
                    _ => fake::not_found(),
                }
            }
            "/api/v1/repos/alice/site/hooks" if m == "GET" => fake::js(&json!(g.repo_hooks)),
            "/api/v1/repos/alice/site/hooks" if m == "POST" => {
                let mut body: Value = serde_json::from_slice(&r.body).unwrap();
                body["id"] = json!(g.repo_hooks.len() + 200);
                g.repo_hooks.push(body.clone());
                fake::created(&body)
            }
            _ => fake::not_found(),
        }
    }

    fn repo_json(id: i64, owner: &str, name: &str) -> Value {
        json!({"id": id, "name": name, "full_name": format!("{owner}/{name}"),
            "owner": {"login": owner}, "private": true, "updated_at": "2026-01-01T00:00:00Z"})
    }

    fn start(kind: &str, st: State) -> (Gitea, TestServer, Arc<Mutex<State>>) {
        let st = Arc::new(Mutex::new(st));
        let s = st.clone();
        let srv = TestServer::start(move |r| route(&s, r));
        let mut g = Gitea::new(&fake::config(kind, &srv.url()));
        g.http.set_sleep(|_| {});
        (g, srv, st)
    }

    #[test]
    fn list_paginates_and_filters_owner() {
        let mut st = State::default();
        for i in 1..=120 {
            st.repos.push(repo_json(i, "alice", &format!("r{i}")));
        }
        st.repos.push(repo_json(999, "stale", "moved"));
        let (p, srv, _) = start("forgejo", st);
        let repos = p.list("alice").unwrap();
        assert_eq!(repos.len(), 120);
        let seen = srv.requests();
        assert!(
            seen.iter()
                .all(|s| s.authorization == format!("token {TOKEN}"))
        );
        let pages = seen
            .iter()
            .filter(|s| s.target.contains("/repos/search"))
            .count();
        assert_eq!(pages, 3);
        assert_eq!(
            srv.lines()[..2],
            [
                "GET /api/v1/users/alice",
                "GET /api/v1/repos/search?limit=50&page=1&uid=1"
            ]
        );
        let again = p.list("alice").unwrap();
        assert!(Arc::ptr_eq(&repos[0], &again[0]));
        assert_eq!(srv.requests().len(), 7);
    }

    #[test]
    fn get() {
        let st = State {
            repos: vec![repo_json(1, "alice", "site")],
            ..Default::default()
        };
        let (p, _srv, _) = start("gitea", st);
        let r = Source::get(&p, "alice", "site").unwrap().unwrap();
        assert_eq!(r.full(), "alice/site");
        assert!(r.private);
        assert_eq!(r.updated, "2026-01-01T00:00:00Z");
        assert!(Source::get(&p, "alice", "missing").unwrap().is_none());
    }

    #[test]
    fn broken_surrogates_do_not_fail_a_listing() {
        let srv = TestServer::start(|r| match r.path() {
            "/api/v1/users/alice" => Reply::json(r#"{"id":1}"#),
            "/api/v1/repos/alice/a" => Reply::json(r#"{"id":1,"name":"a","description":"\udc00"}"#),
            _ => Reply::json(
                r#"{"data":[{"id":1,"name":"a","owner":{"login":"alice"},"description":"bad \ud83d emoji"}]}"#,
            ),
        });
        let p = Gitea::new(&fake::config("forgejo", &srv.url()));
        let repos = p.list("alice").unwrap();
        assert_eq!(repos[0].description, "bad \u{fffd} emoji");
        let r = Source::get(&p, "alice", "a").unwrap().unwrap();
        assert_eq!(r.description, "\u{fffd}");
    }

    #[test]
    fn gogs_listing() {
        let st = State {
            plain: true,
            repos: vec![repo_json(1, "alice", "site")],
            ..Default::default()
        };
        let (p, srv, _) = start("gogs", st);
        let repos = p.list("alice").unwrap();
        assert_eq!(repos.len(), 1);
        assert_eq!(
            srv.lines(),
            [
                "GET /api/v1/orgs/alice/repos?limit=50&page=1",
                "GET /api/v1/users/alice/repos?limit=50&page=1",
                "GET /api/v1/users/alice/repos?limit=50&page=2",
            ]
        );
    }

    #[test]
    fn hooks() {
        let st = State {
            repos: vec![repo_json(1, "alice", "site")],
            ..Default::default()
        };
        let (p, _srv, st) = start("forgejo", st);
        let h = p.hooks().unwrap();
        let (id, res) = h.global(0, false).unwrap();
        assert_eq!(res, "created");
        {
            let g = st.lock().unwrap();
            let cfg = &g.hooks[0]["config"];
            assert_eq!(cfg["url"], "http://127.0.0.1:9001/hook");
            assert_eq!(cfg["secret"], SECRET);
            assert_eq!(g.hooks[0]["type"], "gitea");
            assert_eq!(
                g.hooks[0]["events"],
                json!(["push", "create", "delete", "repository"])
            );
        }
        assert_eq!(h.global(id, false).unwrap(), (id, "exists".into()));
        assert_eq!(h.global(id, true).unwrap().1, "updated");
        let (id3, res) = h.global(0, false).unwrap();
        assert_eq!(res, "created");
        assert_ne!(id3, id);
        assert_eq!(st.lock().unwrap().hooks.len(), 1);
        assert!(h.remove_global(id3));
        assert!(st.lock().unwrap().hooks.is_empty());
        assert!(!h.remove_global(0));
        let repo = Repo {
            owner: "alice".into(),
            name: "site".into(),
            ..Default::default()
        };
        assert_eq!(h.repo(&repo, false).unwrap(), "created");
        assert_eq!(h.repo(&repo, false).unwrap(), "exists");
    }

    #[test]
    fn gogs_remembers_the_owner_kind() {
        let st = State {
            plain: true,
            repos: vec![repo_json(1, "alice", "site")],
            ..Default::default()
        };
        let (p, srv, st) = start("gogs", st);
        p.list("alice").unwrap();
        p.list("alice").unwrap();
        let probes = srv
            .lines()
            .iter()
            .filter(|l| l.starts_with("GET /api/v1/orgs/"))
            .count();
        assert_eq!(probes, 1);
        assert_eq!(srv.requests().len(), 5);
        st.lock().unwrap().plain = false;
        assert!(p.list("alice").is_err());
        st.lock().unwrap().plain = true;
        assert_eq!(p.list("alice").unwrap().len(), 1);
        let probes = srv
            .lines()
            .iter()
            .filter(|l| l.starts_with("GET /api/v1/orgs/"))
            .count();
        assert_eq!(probes, 3);
    }

    #[test]
    fn search_follows_the_total_count_when_the_server_caps_pages() {
        let srv = TestServer::start(|r| match r.path() {
            "/api/v1/users/alice" => fake::js(&json!({"id": 1})),
            _ => {
                let page = atoi(query(&r.target, "page"), 1);
                let lo = (page - 1) * 20;
                let rows: Vec<Value> = (lo..(lo + 20).min(45))
                    .map(|i| repo_json(i as i64 + 1, "alice", &format!("r{i}")))
                    .collect();
                fake::js(&json!({"ok": true, "data": rows})).header("X-Total-Count", "45")
            }
        });
        let p = Gitea::new(&fake::config("forgejo", &srv.url()));
        assert_eq!(p.list("alice").unwrap().len(), 45);
        assert_eq!(srv.requests().len(), 4);

        let srv = TestServer::start(|r| match r.path() {
            "/api/v1/users/alice" => fake::js(&json!({"id": 1})),
            _ => fake::js(&json!({"data": [repo_json(1, "alice", "a")]})),
        });
        let p = Gitea::new(&fake::config("forgejo", &srv.url()));
        assert_eq!(p.list("alice").unwrap().len(), 1);
        assert_eq!(srv.requests().len(), 2);
    }

    #[test]
    fn system_hook_lookup_errors_do_not_create_duplicates() {
        let srv = TestServer::start(|r| match (r.method.as_str(), r.path()) {
            ("GET", "/api/v1/admin/hooks/7") => Reply::status(503).body("down"),
            ("GET", "/api/v1/admin/hooks/8") => Reply::status(403).body("no"),
            _ => fake::created(&json!({"id": 9})),
        });
        let mut p = Gitea::new(&fake::config("forgejo", &srv.url()));
        p.http.set_sleep(|_| {});
        let h = p.hooks().unwrap();
        assert!(h.global(7, false).is_err());
        assert!(h.global(8, false).is_err());
        assert!(srv.lines().iter().all(|l| !l.starts_with("POST")));
    }

    #[test]
    fn system_hook_with_an_unreadable_id_is_cleaned_up() {
        let hooks: Arc<Mutex<Vec<(i64, bool)>>> = Arc::new(Mutex::new(vec![(5, true)]));
        let garbled = Arc::new(Mutex::new(1));
        let (hk, gb) = (hooks.clone(), garbled.clone());
        let srv = TestServer::start(move |r| {
            let mut hooks = hk.lock().unwrap();
            let p = r.path();
            let id: i64 = p
                .strip_prefix("/api/v1/admin/hooks/")
                .and_then(|x| x.parse().ok())
                .unwrap_or(0);
            match r.method.as_str() {
                "POST" => {
                    let next = hooks.iter().map(|h| h.0).max().unwrap_or(0) + 1;
                    hooks.push((next, true));
                    let mut left = gb.lock().unwrap();
                    if *left > 0 {
                        *left -= 1;
                        return Reply::status(201).body("{\"id\":");
                    }
                    fake::created(
                        &json!({"id": next, "config": {"url": "http://127.0.0.1:9001/hook"}}),
                    )
                }
                "GET" if hooks.iter().any(|h| h.0 == id) => {
                    fake::js(&json!({"id": id, "config": {"url": "http://127.0.0.1:9001/hook"}}))
                }
                "PATCH" => fake::js(&json!({})),
                "DELETE" => {
                    hooks.retain(|h| h.0 != id);
                    Reply::status(204)
                }
                _ => fake::not_found(),
            }
        });
        let mut p = Gitea::new(&fake::config("forgejo", &srv.url()));
        p.http.set_sleep(|_| {});
        let h = p.hooks().unwrap();
        assert_eq!(h.global(5, true).unwrap(), (5, "updated".into()));
        assert_eq!(h.global(0, false).unwrap(), (7, "created".into()));
        assert_eq!(*hooks.lock().unwrap(), vec![(7, true)]);

        *garbled.lock().unwrap() = 2;
        assert!(h.global(0, false).is_err());
        assert_eq!(hooks.lock().unwrap().len(), 3);
        assert_eq!(h.global(7, false).unwrap(), (10, "created".into()));
        assert_eq!(*hooks.lock().unwrap(), vec![(10, true)]);
        assert_eq!(h.global(10, false).unwrap(), (10, "exists".into()));
    }

    #[test]
    fn only_forgejo_uses_system_webhooks_by_default() {
        for (kind, want) in [("forgejo", true), ("codeberg", true), ("gitea", false)] {
            let (p, _srv, _) = start(kind, State::default());
            let id = p.whoami().unwrap();
            assert!(id.admin, "{kind}");
            assert_eq!(id.system_hooks, want, "{kind}");
            assert_eq!(id.login, "alice");
        }
        let (p, _srv, _) = start("codeberg", State::default());
        assert_eq!(p.kind(), "forgejo");
    }

    #[test]
    fn git_headers() {
        let (p, _srv, _) = start("forgejo", State::default());
        assert_eq!(
            p.git_header().unwrap(),
            format!("Authorization: token {TOKEN}")
        );
        let (p, srv, _) = start("gogs", State::default());
        let want = format!(
            "Authorization: Basic {}",
            base64(format!("alice:{TOKEN}").as_bytes())
        );
        assert_eq!(p.git_header().unwrap(), want);
        assert_eq!(p.git_header().unwrap(), want);
        assert_eq!(srv.requests().len(), 1);
    }

    #[test]
    fn rejected_token() {
        let srv = TestServer::start(|_| Reply::status(401));
        let mut cfg = fake::config("forgejo", &srv.url());
        cfg.source.kind = "forgejo".into();
        let err = Gitea::new(&cfg).whoami().unwrap_err();
        assert!(err.to_string().contains("rejected"), "{err}");
        assert_eq!(err.status(), Some(401));
    }

    #[test]
    fn owner_exists_only_on_success() {
        let (p, _srv, _) = start("forgejo", State::default());
        assert!(p.owner_exists("alice").unwrap());
        assert!(!p.owner_exists("nobody").unwrap());
    }

    #[test]
    fn role_mapping() {
        for (body, want) in [
            (
                r#"{"id":1,"name":"a","full_name":"o/a","permissions":{"admin":true,"push":true,"pull":true}}"#,
                ROLE_ADMIN,
            ),
            (
                r#"{"id":1,"name":"a","full_name":"o/a","permissions":{"admin":false,"push":true,"pull":true}}"#,
                ROLE_WRITE,
            ),
            (
                r#"{"id":1,"name":"a","full_name":"o/a","permissions":{"admin":false,"push":false,"pull":true}}"#,
                ROLE_READ,
            ),
            (r#"{"id":1,"name":"a","full_name":"o/a"}"#, ""),
        ] {
            let g: GiteaRepo<'_> = serde_json::from_str(body).unwrap();
            assert_eq!(g.into_repo().role, want, "{body}");
        }
    }

    #[test]
    fn owner_and_name_fall_back_to_the_full_name() {
        let g: GiteaRepo<'_> = serde_json::from_str(
            r#"{"id":3,"full_name":"o/a","owner":null,"description":" d ","topics":null}"#,
        )
        .unwrap();
        let r = g.into_repo();
        assert_eq!((r.owner.as_str(), r.name.as_str()), ("o", "a"));
        assert_eq!(r.description, "d");
        let g: GiteaRepo<'_> =
            serde_json::from_str(r#"{"name":"a","owner":{"username":"u"}}"#).unwrap();
        assert_eq!(g.into_repo().owner, "u");
    }

    #[test]
    fn webhook_parsing() {
        let (p, _srv, _) = start("forgejo", State::default());
        let body = r#"{"action":"deleted","repository":{"id":7,"full_name":"alice/site"}}"#;
        let sig = mac("s3cret", body);
        for name in [
            "X-Gitea-Signature",
            "X-Forgejo-Signature",
            "X-Gogs-Signature",
        ] {
            let h = fake::headers(&[(name, &sig), ("X-Gitea-Event", "push")]);
            fake::must_event(
                p.parse_webhook(&h, body.as_bytes(), "s3cret"),
                Change::Changed,
                "alice",
                "site",
                7,
            );
        }
        let h = fake::headers(&[
            ("X-Hub-Signature-256", &format!("sha256={sig}")),
            ("X-Gogs-Event", "push"),
        ]);
        assert!(
            p.parse_webhook(&h, body.as_bytes(), "s3cret")
                .unwrap()
                .is_some()
        );
        let h = fake::headers(&[("X-Gitea-Signature", &sig), ("X-Gitea-Event", "repository")]);
        fake::must_event(
            p.parse_webhook(&h, body.as_bytes(), "s3cret"),
            Change::Removed,
            "alice",
            "site",
            7,
        );
        let h = fake::headers(&[("X-Gitea-Signature", &sig), ("X-Gitea-Event", "issues")]);
        assert!(
            p.parse_webhook(&h, body.as_bytes(), "s3cret")
                .unwrap()
                .is_none()
        );
        let zeros = "0".repeat(64);
        let other = mac("other", body);
        for h in [
            fake::headers(&[]),
            fake::headers(&[("X-Gitea-Signature", &zeros)]),
            fake::headers(&[("X-Gitea-Signature", &other)]),
        ] {
            assert_eq!(
                p.parse_webhook(&h, body.as_bytes(), "s3cret"),
                Err(Error::Signature)
            );
        }
    }
}
