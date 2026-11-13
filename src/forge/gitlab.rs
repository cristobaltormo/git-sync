use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use serde::Deserialize;
use serde::de::IgnoredAny;
use serde_json::{Value, json};

use super::common::{
    Common, Text, check, cut, decode_with, deleted, nullable, path_escape, rejected, tolerant,
};
use super::{Change, Event, Hooks, Identity, PageCache, Provider, Repo, Source, constant_eq};
use crate::config::Config;
use crate::error::Error;
use crate::httpx::{Client, Headers, Response, fail};

pub struct GitLab {
    c: Common,
    base: String,
    http: Client,
    pages: PageCache,
    ns: Mutex<HashMap<String, Namespace>>,
}

#[derive(Clone, Copy)]
struct Namespace {
    id: i64,
    user: bool,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct NamespaceRef<'a> {
    #[serde(borrow)]
    full_path: Text<'a>,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct Project<'a> {
    #[serde(deserialize_with = "nullable")]
    id: i64,
    #[serde(borrow)]
    path: Text<'a>,
    #[serde(borrow)]
    path_with_namespace: Text<'a>,
    #[serde(borrow)]
    description: Text<'a>,
    #[serde(borrow)]
    visibility: Text<'a>,
    #[serde(deserialize_with = "nullable")]
    archived: bool,
    #[serde(borrow)]
    default_branch: Text<'a>,
    #[serde(deserialize_with = "nullable")]
    empty_repo: bool,
    #[serde(deserialize_with = "nullable")]
    mirror: bool,
    #[serde(deserialize_with = "nullable")]
    topics: Vec<String>,
    #[serde(borrow)]
    last_activity_at: Text<'a>,
    #[serde(borrow, rename = "marked_for_deletion_at")]
    marked_for_deletion: Text<'a>,
    #[serde(rename = "forked_from_project")]
    forked_from: Option<IgnoredAny>,
    #[serde(borrow)]
    namespace: Option<NamespaceRef<'a>>,
}

impl Project<'_> {
    fn into_repo(self) -> Repo {
        let owner = match &self.namespace {
            Some(n) if !n.full_path.is_empty() => n.full_path.as_str(),
            _ => cut(self.path_with_namespace.as_str(), '/').0,
        }
        .to_string();
        Repo {
            id: self.id,
            owner,
            name: self.path.into_string(),
            private: self.visibility.as_str() != "public",
            description: self.description.trimmed(),
            archived: self.archived,
            default_branch: self.default_branch.into_string(),
            fork: self.forked_from.is_some(),
            mirror: self.mirror,
            empty: self.empty_repo,
            topics: self.topics,
            updated: self.last_activity_at.into_string(),
            ..Default::default()
        }
    }
}

fn decode_projects(b: &[u8]) -> Result<Vec<Repo>, Error> {
    let rows: Option<Vec<Project<'_>>> = serde_json::from_slice(b)?;
    Ok(rows
        .unwrap_or_default()
        .into_iter()
        .filter(|p| p.marked_for_deletion.is_empty())
        .map(Project::into_repo)
        .collect())
}

impl GitLab {
    pub fn new(cfg: &Config) -> GitLab {
        let c = Common::new(cfg);
        GitLab {
            base: format!("{}/api/v4", c.url),
            http: c.client("Bearer"),
            pages: PageCache::default(),
            ns: Mutex::new(HashMap::new()),
            c,
        }
    }

    fn fetch(&self, path: &str) -> Result<Response, Error> {
        self.http.get(&format!("{}{path}", self.base))
    }

    fn namespace(&self, path: &str) -> Result<Namespace, Error> {
        if let Some(n) = self.ns.lock().unwrap_or_else(|e| e.into_inner()).get(path) {
            return Ok(*n);
        }
        let r = self.fetch(&format!("/namespaces/{}", path_escape(path)))?;
        if !r.ok() {
            return Err(Error::api(
                r.status,
                format!("cannot look up namespace '{path}'"),
            ));
        }
        #[derive(Deserialize, Default)]
        #[serde(default)]
        struct V<'a> {
            #[serde(deserialize_with = "nullable")]
            id: i64,
            #[serde(borrow)]
            kind: Text<'a>,
        }
        let v: V<'_> = r.decode()?;
        let n = Namespace {
            id: v.id,
            user: v.kind.as_str() == "user",
        };
        self.ns
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(path.to_string(), n);
        Ok(n)
    }

    fn project_hooks(r: &Repo) -> String {
        format!(
            "/projects/{}/hooks",
            path_escape(&format!("{}/{}", r.owner, r.name))
        )
    }

    fn hook_body(&self) -> Value {
        json!({
            "url": self.c.hook_url,
            "token": self.c.secret,
            "push_events": true,
            "tag_push_events": true,
            "repository_update_events": true,
            "enable_ssl_verification": true,
        })
    }

    fn ours(&self, path: &str) -> Result<Vec<i64>, Error> {
        let r = self.fetch(&format!("{path}?per_page=100"))?;
        if !r.ok() {
            return Err(fail(&r, path));
        }
        #[derive(Deserialize, Default)]
        #[serde(default)]
        struct H<'a> {
            #[serde(deserialize_with = "nullable")]
            id: i64,
            #[serde(borrow)]
            url: Text<'a>,
        }
        let all: Option<Vec<H<'_>>> = r.decode()?;
        Ok(all
            .unwrap_or_default()
            .iter()
            .filter(|x| x.url.as_str() == self.c.hook_url)
            .map(|x| x.id)
            .collect())
    }

    fn ensure(&self, path: &str, force: bool) -> Result<(i64, String), Error> {
        let mine = self.ours(path)?;
        if let Some(id) = mine.first()
            && !force
        {
            return Ok((*id, "exists".into()));
        }
        let (method, target, result) = match mine.first() {
            Some(id) => ("PUT", format!("{}{path}/{id}", self.base), "updated"),
            None => ("POST", format!("{}{path}", self.base), "created"),
        };
        let r = check(self.http.json(method, &target, &self.hook_body())?)?;
        #[derive(Deserialize, Default)]
        #[serde(default)]
        struct Created {
            #[serde(deserialize_with = "nullable")]
            id: i64,
        }
        let id = r.decode::<Created>().map(|c| c.id).unwrap_or(0);
        Ok((id, result.into()))
    }

    fn remove_all(&self, path: &str) -> i64 {
        let Ok(mine) = self.ours(path) else {
            return 0;
        };
        mine.iter()
            .filter(|id| deleted(&self.http, &format!("{}{path}/{id}", self.base)))
            .count() as i64
    }
}

impl Source for GitLab {
    fn kind(&self) -> &'static str {
        "gitlab"
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
            username: Text<'a>,
            #[serde(deserialize_with = "nullable")]
            is_admin: bool,
        }
        let u: U<'_> = r.decode()?;
        Ok(Identity {
            login: u.username.into_string(),
            admin: u.is_admin,
            system_hooks: u.is_admin,
        })
    }

    fn list(&self, owner: &str) -> Result<Vec<Arc<Repo>>, Error> {
        let ns = self.namespace(owner)?;
        let (base, extra) = if ns.user {
            (format!("/users/{}/projects", ns.id), "")
        } else {
            (
                format!("/groups/{}/projects", ns.id),
                "include_subgroups=false&",
            )
        };
        let mut out = Vec::new();
        let mut n = 1;
        loop {
            let q = format!("{extra}page={n}&per_page=100");
            let r = self.fetch(&format!("{base}?{q}"))?;
            if !r.ok() {
                return Err(fail(&r, &format!("listing {owner}")));
            }
            let page = self.pages.decode(&format!("{base}?{q}"), &r.body, |b| {
                tolerant(b, decode_projects)
            })?;
            out.extend(page.iter().cloned());
            if r.headers.get("X-Next-Page").is_empty() {
                return Ok(out);
            }
            n += 1;
        }
    }

    fn get(&self, owner: &str, name: &str) -> Result<Option<Arc<Repo>>, Error> {
        let r = self.fetch(&format!(
            "/projects/{}",
            path_escape(&format!("{owner}/{name}"))
        ))?;
        if r.status == 404 {
            return Ok(None);
        }
        let r = check(r)?;
        let repo = decode_with(&r, |b| {
            let p: Project<'_> = serde_json::from_slice(b)?;
            Ok(p.marked_for_deletion.is_empty().then(|| p.into_repo()))
        })?;
        Ok(repo.map(Arc::new))
    }

    fn owner_exists(&self, owner: &str) -> Result<bool, Error> {
        Ok(self
            .fetch(&format!("/namespaces/{}", path_escape(owner)))?
            .ok())
    }

    fn hooks(&self) -> Option<&dyn Hooks> {
        Some(self)
    }
}

impl Provider for GitLab {
    fn clone_url(&self, r: &Repo) -> String {
        format!("{}/{}/{}.git", self.c.url, r.owner, r.name)
    }

    fn git_header(&self) -> Result<String, Error> {
        Ok(self.c.basic_header("oauth2"))
    }

    fn parse_webhook(
        &self,
        h: &Headers,
        body: &[u8],
        secret: &str,
    ) -> Result<Option<Event>, Error> {
        if !constant_eq(h.get("X-Gitlab-Token").as_bytes(), secret.as_bytes()) {
            return Err(Error::Signature);
        }
        #[derive(Deserialize, Default)]
        #[serde(default)]
        struct Proj<'a> {
            #[serde(deserialize_with = "nullable")]
            id: i64,
            #[serde(borrow)]
            path_with_namespace: Text<'a>,
        }
        #[derive(Deserialize, Default)]
        #[serde(default)]
        struct P<'a> {
            #[serde(borrow)]
            event_name: Text<'a>,
            #[serde(deserialize_with = "nullable")]
            project_id: i64,
            #[serde(borrow)]
            path_with_namespace: Text<'a>,
            #[serde(borrow)]
            project: Option<Proj<'a>>,
        }
        tolerant(body, |b| {
            let p: P<'_> = serde_json::from_slice(b)?;
            let (path, id) = match &p.project {
                Some(x) if !x.path_with_namespace.is_empty() => {
                    (x.path_with_namespace.as_str(), x.id)
                }
                _ => (p.path_with_namespace.as_str(), p.project_id),
            };
            let i = match path.rfind('/') {
                Some(i) if i >= 1 => i,
                _ => return Ok(None),
            };
            let kind = if p.event_name.as_str() == "project_destroy" {
                Change::Removed
            } else {
                Change::Changed
            };
            Ok(Some(Event {
                kind,
                owner: path[..i].to_string(),
                name: path[i + 1..].to_string(),
                id,
            }))
        })
    }
}

impl Hooks for GitLab {
    fn global(&self, _known: i64, force: bool) -> Result<(i64, String), Error> {
        self.ensure("/hooks", force)
    }

    fn repo(&self, r: &Repo, force: bool) -> Result<String, Error> {
        Ok(self.ensure(&Self::project_hooks(r), force)?.1)
    }

    fn remove_global(&self, _id: i64) -> bool {
        self.remove_all("/hooks") > 0
    }

    fn remove_repo(&self, r: &Repo) -> i64 {
        self.remove_all(&Self::project_hooks(r))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::forge::common::fake::{self, SECRET, TOKEN, query};
    use crate::httpx::base64;
    use crate::testutil::{Reply, TestServer};
    use crate::webhook::Request;

    fn project(id: i64, path: &str, ns: &str, vis: &str) -> Value {
        json!({"id": id, "path": path, "path_with_namespace": format!("{ns}/{path}"),
            "description": "d", "visibility": vis, "archived": false, "default_branch": "main",
            "empty_repo": false, "mirror": null, "topics": ["a"],
            "last_activity_at": "2026-09-24T19:29:15.121Z", "namespace": {"full_path": ns}})
    }

    fn route(hooks: &Mutex<Vec<Value>>, r: &Request) -> Reply {
        let p = r.path();
        let m = r.method.as_str();
        let gone = || {
            let mut g = project(9, "old-deletion_scheduled-9", "root", "private");
            g["marked_for_deletion_at"] = json!("2026-09-24");
            g
        };
        let hook_list = p == "/api/v4/hooks" || p == "/api/v4/projects/root%2Fsite/hooks";
        match p {
            "/api/v4/user" => fake::js(&json!({"username": "root", "is_admin": true})),
            "/api/v4/namespaces/root" => fake::js(&json!({"id": 1, "kind": "user"})),
            "/api/v4/namespaces/team%2Fweb" => fake::js(&json!({"id": 5, "kind": "group"})),
            "/api/v4/users/1/projects" => fake::js(&json!([
                project(1, "site", "root", "private"),
                project(2, "blog", "root", "internal"),
                gone()
            ])),
            "/api/v4/projects/root%2Fold-deletion_scheduled-9" => fake::js(&gone()),
            "/api/v4/groups/5/projects" => {
                if query(&r.target, "page") == "1" {
                    fake::js(&json!([project(3, "a", "team/web", "public")]))
                        .header("X-Next-Page", "2")
                } else {
                    fake::js(&json!([project(4, "b", "team/web", "public")]))
                }
            }
            "/api/v4/projects/root%2Fsite" => fake::js(&project(1, "site", "root", "public")),
            _ if hook_list && m == "GET" => fake::js(&json!(*hooks.lock().unwrap())),
            _ if hook_list && m == "POST" => {
                let mut h: Value = serde_json::from_slice(&r.body).unwrap();
                let mut all = hooks.lock().unwrap();
                h["id"] = json!(all.len() + 10);
                all.push(h.clone());
                fake::created(&h)
            }
            _ if m == "DELETE"
                && (p.starts_with("/api/v4/hooks/")
                    || p.starts_with("/api/v4/projects/root%2Fsite/hooks/")) =>
            {
                hooks.lock().unwrap().clear();
                Reply::status(204)
            }
            _ => fake::not_found(),
        }
    }

    fn start() -> (GitLab, TestServer, Arc<Mutex<Vec<Value>>>) {
        let hooks = Arc::new(Mutex::new(Vec::new()));
        let h = hooks.clone();
        let srv = TestServer::start(move |r| route(&h, r));
        let mut g = GitLab::new(&fake::config("gitlab", &srv.url()));
        g.http.set_sleep(|_| {});
        (g, srv, hooks)
    }

    #[test]
    fn gitlab() {
        let (g, srv, hooks) = start();
        let id = g.whoami().unwrap();
        assert_eq!(id.login, "root");
        assert!(id.admin && id.system_hooks);
        assert_eq!(srv.requests()[0].authorization, format!("Bearer {TOKEN}"));

        let repos = g.list("root").unwrap();
        assert_eq!(repos.len(), 2);
        assert!(repos[0].private && repos[1].private);
        assert_eq!(
            (repos[0].name.as_str(), repos[0].owner.as_str()),
            ("site", "root")
        );
        assert_eq!(repos[0].default_branch, "main");
        assert!(!repos[0].updated.is_empty());
        assert_eq!(repos[0].topics, vec!["a"]);
        assert!(!repos[0].mirror && !repos[0].fork);

        let group = g.list("team/web").unwrap();
        assert_eq!(group.len(), 2);
        assert!(!group[0].private);
        assert_eq!(group[0].owner, "team/web");
        assert!(srv.lines().contains(
            &"GET /api/v4/groups/5/projects?include_subgroups=false&page=2&per_page=100".into()
        ));

        let r = Source::get(&g, "root", "site").unwrap().unwrap();
        assert!(!r.private);
        assert!(
            Source::get(&g, "root", "old-deletion_scheduled-9")
                .unwrap()
                .is_none()
        );
        assert!(Source::get(&g, "root", "gone").unwrap().is_none());
        assert_eq!(
            g.git_header().unwrap(),
            format!(
                "Authorization: Basic {}",
                base64(format!("oauth2:{TOKEN}").as_bytes())
            )
        );
        let team = Repo {
            owner: "team/web".into(),
            name: "a".into(),
            ..Default::default()
        };
        assert!(g.clone_url(&team).ends_with("/team/web/a.git"));
        assert!(g.owner_exists("team/web").unwrap());
        assert!(!g.owner_exists("nobody").unwrap());

        let h = g.hooks().unwrap();
        let (id, res) = h.global(0, false).unwrap();
        assert_eq!((id, res.as_str()), (10, "created"));
        assert_eq!(h.global(0, false).unwrap(), (10, "exists".into()));
        {
            let all = hooks.lock().unwrap();
            assert_eq!(all[0]["token"], SECRET);
            assert_eq!(all[0]["tag_push_events"], true);
        }
        assert!(h.remove_global(0));
        assert!(hooks.lock().unwrap().is_empty());
        let site = Repo {
            owner: "root".into(),
            name: "site".into(),
            ..Default::default()
        };
        assert_eq!(h.repo(&site, false).unwrap(), "created");
        assert_eq!(h.remove_repo(&site), 1);
    }

    #[test]
    fn webhooks() {
        let srv = TestServer::start(|_| Reply::status(200));
        let g = GitLab::new(&fake::config("gitlab", &srv.url()));
        let tok = fake::headers(&[("X-Gitlab-Token", SECRET)]);
        let push = r#"{"object_kind":"push","project_id":7,"project":{"id":7,"path_with_namespace":"team/web/site"}}"#;
        fake::must_event(
            g.parse_webhook(&tok, push.as_bytes(), SECRET),
            Change::Changed,
            "team/web",
            "site",
            7,
        );
        let system =
            r#"{"event_name":"project_destroy","project_id":9,"path_with_namespace":"root/old"}"#;
        fake::must_event(
            g.parse_webhook(&tok, system.as_bytes(), SECRET),
            Change::Removed,
            "root",
            "old",
            9,
        );
        assert!(
            g.parse_webhook(&tok, br#"{"path_with_namespace":"/x"}"#, SECRET)
                .unwrap()
                .is_none()
        );
        for h in [
            fake::headers(&[]),
            fake::headers(&[("X-Gitlab-Token", "nope")]),
        ] {
            assert_eq!(
                g.parse_webhook(&h, push.as_bytes(), SECRET),
                Err(Error::Signature)
            );
        }
        assert!(srv.requests().is_empty());
    }

    #[test]
    fn request_failures_surface() {
        let srv = TestServer::start(|_| Reply::status(401));
        let err = GitLab::new(&fake::config("gitlab", &srv.url()))
            .whoami()
            .unwrap_err();
        assert!(err.to_string().contains("rejected"), "{err}");
    }
}
