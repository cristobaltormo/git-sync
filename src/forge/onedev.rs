use std::borrow::Cow;
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use serde::Deserialize;
use serde_json::json;
use serde_json::value::{RawValue, to_raw_value};

use super::common::{
    Common, Text, check, decode_with, eq_fold, nullable, path_escape, rejected, tolerant,
};
use super::gitbucket::NO_GLOBAL_HOOKS;
use super::{
    Change, Event, Hooks, Identity, ONEDEV_ROOT, PageCache, Provider, Repo, Source, constant_eq,
};
use crate::config::Config;
use crate::error::Error;
use crate::httpx::{Client, Headers, Response, fail};

const COUNT: usize = 100;

type Raw = Box<RawValue>;
type Setting = (i64, BTreeMap<String, Raw>, Vec<Raw>);

pub struct OneDev {
    c: Common,
    base: String,
    http: Client,
    pages: PageCache,
    login: Mutex<String>,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct Project<'a> {
    #[serde(deserialize_with = "nullable")]
    id: i64,
    #[serde(borrow)]
    name: Text<'a>,
    #[serde(borrow)]
    path: Text<'a>,
    #[serde(borrow)]
    description: Text<'a>,
    #[serde(rename = "forkedFromId")]
    forked_from_id: Option<i64>,
}

impl Project<'_> {
    fn into_repo(self) -> Repo {
        let path = self.path.as_str();
        let owner = match path.rfind('/') {
            Some(i) => &path[..i],
            None => ONEDEV_ROOT,
        }
        .to_string();
        Repo {
            id: self.id,
            owner,
            name: self.name.into_string(),
            private: true,
            description: self.description.trimmed(),
            fork: self.forked_from_id.is_some(),
            ..Default::default()
        }
    }
}

fn decode_projects(b: &[u8]) -> Result<Vec<Repo>, Error> {
    let rows: Option<Vec<Project<'_>>> = serde_json::from_slice(b)?;
    Ok(rows
        .unwrap_or_default()
        .into_iter()
        .map(Project::into_repo)
        .collect())
}

fn project_path<'a>(owner: &str, name: &'a str) -> Cow<'a, str> {
    if owner == ONEDEV_ROOT {
        Cow::Borrowed(name)
    } else {
        Cow::Owned(format!("{owner}/{name}"))
    }
}

fn escape_path(path: &str) -> String {
    path.split('/')
        .map(path_escape)
        .collect::<Vec<_>>()
        .join("/")
}

impl OneDev {
    pub fn new(cfg: &Config) -> OneDev {
        let c = Common::new(cfg);
        OneDev {
            base: format!("{}/~api", c.url),
            http: c.client("Bearer"),
            pages: PageCache::default(),
            login: Mutex::new(String::new()),
            c,
        }
    }

    fn fetch(&self, path: &str) -> Result<Response, Error> {
        self.http.get(&format!("{}{path}", self.base))
    }

    fn id_of(&self, path: &str) -> Result<Option<i64>, Error> {
        let r = self.fetch(&format!("/projects/ids/{}", escape_path(path)))?;
        if r.status == 404 {
            return Ok(None);
        }
        let r = check(r)?;
        let text = std::str::from_utf8(&r.body).unwrap_or("").trim();
        if text.is_empty() || text == "null" {
            return Ok(None);
        }
        match text.parse() {
            Ok(id) => Ok(Some(id)),
            Err(_) => Err(crate::err!(
                "unexpected answer looking up project {path}: {}",
                r.text()
            )),
        }
    }

    fn by_id(&self, id: i64) -> Result<Option<Repo>, Error> {
        let r = self.fetch(&format!("/projects/{id}"))?;
        if r.status == 404 {
            return Ok(None);
        }
        let r = check(r)?;
        let repo = decode_with(&r, |b| {
            Ok(serde_json::from_slice::<Project<'_>>(b)?.into_repo())
        })?;
        Ok(Some(repo))
    }

    fn setting(&self, r: &Repo) -> Result<Setting, Error> {
        let Some(id) = self.id_of(&project_path(&r.owner, &r.name))? else {
            return Err(crate::err!("project {} not found", r.full()));
        };
        let resp = check(self.fetch(&format!("/projects/{id}/setting"))?)?;
        let mut raw: BTreeMap<String, Raw> =
            tolerant(&resp.body, |b| Ok(serde_json::from_slice(b)?))?;
        let hooks = match raw.remove("webHooks") {
            Some(v) => serde_json::from_str::<Option<Vec<Raw>>>(v.get())
                .ok()
                .flatten()
                .unwrap_or_default(),
            None => Vec::new(),
        };
        Ok((id, raw, hooks))
    }

    fn save(&self, id: i64, mut raw: BTreeMap<String, Raw>, hooks: Vec<Raw>) -> Result<(), Error> {
        raw.insert("webHooks".into(), to_raw_value(&hooks)?);
        check(self.http.json(
            "POST",
            &format!("{}/projects/{id}/setting", self.base),
            &raw,
        )?)?;
        Ok(())
    }

    fn is_ours(&self, hook: &RawValue) -> bool {
        #[derive(Deserialize, Default)]
        #[serde(default)]
        struct H<'a> {
            #[serde(borrow, rename = "postUrl")]
            post_url: Text<'a>,
        }
        serde_json::from_str::<H<'_>>(hook.get())
            .is_ok_and(|h| h.post_url.as_str() == self.c.hook_url)
    }
}

impl Source for OneDev {
    fn kind(&self) -> &'static str {
        "onedev"
    }

    fn whoami(&self) -> Result<Identity, Error> {
        let r = self.fetch("/users/me")?;
        if r.status == 401 {
            return Err(rejected());
        }
        let r = check(r)?;
        #[derive(Deserialize, Default)]
        #[serde(default)]
        struct U<'a> {
            #[serde(borrow)]
            name: Text<'a>,
        }
        let u: U<'_> = r.decode()?;
        let login = u.name.into_string();
        *self.login.lock().unwrap_or_else(|e| e.into_inner()) = login.clone();
        Ok(Identity {
            login,
            ..Default::default()
        })
    }

    fn list(&self, owner: &str) -> Result<Vec<Arc<Repo>>, Error> {
        let mut out = Vec::new();
        let mut offset = 0;
        loop {
            let r = self.fetch(&format!("/projects?offset={offset}&count={COUNT}"))?;
            if !r.ok() {
                return Err(fail(&r, "listing projects"));
            }
            let page = self
                .pages
                .decode(&format!("projects/{offset}"), &r.body, |b| {
                    tolerant(b, decode_projects)
                })?;
            out.extend(page.iter().filter(|x| eq_fold(&x.owner, owner)).cloned());
            if page.len() < COUNT {
                return Ok(out);
            }
            offset += COUNT;
        }
    }

    fn get(&self, owner: &str, name: &str) -> Result<Option<Arc<Repo>>, Error> {
        match self.id_of(&project_path(owner, name))? {
            Some(id) => Ok(self.by_id(id)?.map(Arc::new)),
            None => Ok(None),
        }
    }

    fn owner_exists(&self, owner: &str) -> Result<bool, Error> {
        if owner == ONEDEV_ROOT {
            return Ok(true);
        }
        Ok(self.id_of(owner)?.is_some())
    }

    fn hooks(&self) -> Option<&dyn Hooks> {
        Some(self)
    }
}

impl Provider for OneDev {
    fn clone_url(&self, r: &Repo) -> String {
        format!("{}/{}.git", self.c.url, project_path(&r.owner, &r.name))
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
        if !constant_eq(h.get("X-OneDev-Signature").as_bytes(), secret.as_bytes()) {
            return Err(Error::Signature);
        }
        #[derive(Deserialize, Default)]
        #[serde(default)]
        struct P {
            #[serde(rename = "projectId", deserialize_with = "nullable")]
            project_id: i64,
        }
        let p: P = tolerant(body, |b| Ok(serde_json::from_slice(b)?))?;
        if p.project_id == 0 {
            return Ok(None);
        }
        Ok(self.by_id(p.project_id)?.map(|r| Event {
            kind: Change::Changed,
            owner: r.owner,
            name: r.name,
            id: r.id,
        }))
    }
}

impl Hooks for OneDev {
    fn global(&self, _known: i64, _force: bool) -> Result<(i64, String), Error> {
        Err(Error::other(NO_GLOBAL_HOOKS))
    }

    fn repo(&self, r: &Repo, force: bool) -> Result<String, Error> {
        let (id, raw, mut hooks) = self.setting(r)?;
        let entry = to_raw_value(&json!({
            "postUrl": self.c.hook_url,
            "secret": self.c.secret,
            "eventTypes": ["CODE_PUSH"],
            "headers": [],
        }))?;
        if let Some(i) = hooks.iter().position(|x| self.is_ours(x)) {
            if !force {
                return Ok("exists".into());
            }
            hooks[i] = entry;
            self.save(id, raw, hooks)?;
            return Ok("updated".into());
        }
        hooks.push(entry);
        self.save(id, raw, hooks)?;
        Ok("created".into())
    }

    fn remove_global(&self, _id: i64) -> bool {
        false
    }

    fn remove_repo(&self, r: &Repo) -> i64 {
        let Ok((id, raw, hooks)) = self.setting(r) else {
            return 0;
        };
        let total = hooks.len();
        let keep: Vec<Raw> = hooks.into_iter().filter(|x| !self.is_ours(x)).collect();
        let removed = total - keep.len();
        if removed == 0 || self.save(id, raw, keep).is_err() {
            return 0;
        }
        removed as i64
    }
}

#[cfg(test)]
mod tests {
    use serde_json::Value;

    use super::*;
    use crate::forge::common::fake::{self, TOKEN};
    use crate::testutil::{Reply, TestServer};

    fn project(id: i64, name: &str, path: &str, forked: Value) -> Value {
        json!({"id": id, "name": name, "path": path, "description": "hola",
            "forkedFromId": forked, "parentId": null})
    }

    #[test]
    fn onedev() {
        let setting = Arc::new(Mutex::new(json!({
            "branchProtections": [],
            "webHooks": [{"postUrl": "http://other/hook", "secret": "x", "eventTypes": ["PULL_REQUEST"],
                "headers": [], "extra": {"kept": true}}]
        })));
        let st = setting.clone();
        let srv = TestServer::start(move |r| {
            let m = r.method.as_str();
            match r.path() {
                "/~api/users/me" => fake::js(&json!({"id": 1, "name": "admin"})),
                "/~api/projects" => fake::js(&json!([
                    project(1, "p1", "p1", Value::Null),
                    project(2, "child", "parent/child", json!(1)),
                    project(3, "other", "parent/other", Value::Null)
                ])),
                "/~api/projects/ids/p1" => Reply::status(200).body("1"),
                "/~api/projects/ids/parent/child" => Reply::status(200).body("2"),
                p if p.starts_with("/~api/projects/ids/") => fake::not_found(),
                "/~api/projects/2" => fake::js(&project(2, "child", "parent/child", json!(1))),
                "/~api/projects/1/setting" if m == "GET" => fake::js(&st.lock().unwrap()),
                "/~api/projects/1/setting" if m == "POST" => {
                    *st.lock().unwrap() = serde_json::from_slice(&r.body).unwrap();
                    fake::js(&json!(1))
                }
                _ => fake::not_found(),
            }
        });
        let cfg = fake::config("onedev", &srv.url());
        let mut o = OneDev::new(&cfg);
        o.http.set_sleep(|_| {});

        let root = o.list(ONEDEV_ROOT).unwrap();
        assert_eq!(root.len(), 1);
        assert_eq!(root[0].name, "p1");
        assert_eq!(root[0].owner, ONEDEV_ROOT);
        assert_eq!(root[0].full(), "p1");
        let child = o.list("parent").unwrap();
        assert_eq!(child.len(), 2);
        assert!(child[0].fork && child[0].private);
        assert_eq!(child[0].owner, "parent");
        assert_eq!(srv.requests()[0].authorization, format!("Bearer {TOKEN}"));
        assert_eq!(
            srv.requests()[0].target,
            "/~api/projects?offset=0&count=100"
        );

        assert_eq!(Source::get(&o, "parent", "child").unwrap().unwrap().id, 2);
        assert!(Source::get(&o, "parent", "missing").unwrap().is_none());
        assert!(o.owner_exists(ONEDEV_ROOT).unwrap());
        assert!(!o.owner_exists("nobody").unwrap());
        let nested = Repo {
            owner: "parent".into(),
            name: "child".into(),
            ..Default::default()
        };
        assert!(o.clone_url(&nested).ends_with("/parent/child.git"));
        let repo = Repo {
            owner: ONEDEV_ROOT.into(),
            name: "p1".into(),
            ..Default::default()
        };
        let u = o.clone_url(&repo);
        assert!(u.ends_with("/p1.git") && !u.contains("//p1"), "{u}");

        let h = o.hooks().unwrap();
        assert_eq!(h.global(0, false).unwrap_err().to_string(), NO_GLOBAL_HOOKS);
        assert_eq!(h.repo(&repo, false).unwrap(), "created");
        {
            let s = setting.lock().unwrap();
            let stored = s["webHooks"].as_array().unwrap();
            assert_eq!(stored.len(), 2);
            assert_eq!(stored[0]["extra"]["kept"], true);
            assert_eq!(stored[1]["postUrl"], cfg.hook_url().as_str());
            assert_eq!(stored[1]["secret"], cfg.listen.secret.as_str());
            assert_eq!(stored[1]["eventTypes"][0], "CODE_PUSH");
            assert!(s.get("branchProtections").is_some());
        }
        assert_eq!(h.repo(&repo, false).unwrap(), "exists");
        assert_eq!(h.repo(&repo, true).unwrap(), "updated");
        assert_eq!(h.remove_repo(&repo), 1);
        assert_eq!(h.remove_repo(&repo), 0);
        assert_eq!(
            setting.lock().unwrap()["webHooks"]
                .as_array()
                .unwrap()
                .len(),
            1
        );
    }

    #[test]
    fn webhooks() {
        let srv = TestServer::start(|_| {
            fake::js(&json!({"id": 2, "name": "child", "path": "parent/child"}))
        });
        let o = OneDev::new(&fake::config("onedev", &srv.url()));
        let body = br#"{"projectId":2,"refName":"refs/heads/main","type":"RefUpdated"}"#;
        let signed = |v: &str| fake::headers(&[("X-OneDev-Signature", v)]);
        fake::must_event(
            o.parse_webhook(&signed("s3cret"), body, "s3cret"),
            Change::Changed,
            "parent",
            "child",
            2,
        );
        assert_eq!(
            o.parse_webhook(&signed("nope"), body, "s3cret"),
            Err(Error::Signature)
        );
        assert!(
            o.parse_webhook(&signed("s3cret"), b"{}", "s3cret")
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn lookups_fail_closed_unless_the_project_is_really_gone() {
        let srv = TestServer::start(|r| match r.path() {
            "/~api/projects/ids/gone" => fake::not_found(),
            "/~api/projects/ids/empty" => Reply::status(204),
            "/~api/projects/ids/null" => Reply::status(200).body("null"),
            "/~api/projects/ids/denied" | "/~api/projects/ids/team" => Reply::status(401),
            "/~api/projects/ids/forbidden" => Reply::status(403),
            "/~api/projects/ids/broken" => Reply::status(500).body("boom"),
            "/~api/projects/ids/html" => Reply::status(200).body("<html>login</html>"),
            _ => fake::not_found(),
        });
        let mut o = OneDev::new(&fake::config("onedev", &srv.url()));
        o.http.set_sleep(|_| {});
        for name in ["gone", "empty", "null"] {
            assert!(
                Source::get(&o, ONEDEV_ROOT, name).unwrap().is_none(),
                "{name}"
            );
        }
        for name in ["denied", "forbidden", "broken", "html"] {
            assert!(Source::get(&o, ONEDEV_ROOT, name).is_err(), "{name}");
        }
        assert!(o.owner_exists("team").is_err());
        assert!(!o.owner_exists("gone").unwrap());
    }

    #[test]
    fn settings_round_trip_keeps_untouched_values_byte_exact() {
        let posted = Arc::new(Mutex::new(Vec::new()));
        let p = posted.clone();
        let srv = TestServer::start(move |r| match (r.method.as_str(), r.path()) {
            ("GET", "/~api/projects/ids/p1") => Reply::status(200).body("1"),
            ("GET", "/~api/projects/1/setting") => Reply::json(
                r#"{"zeta":{"big":123456789012345678901234567890,"f":1e3,"neg":-0.0},"alpha":[1.50],"webHooks":null}"#,
            ),
            ("POST", "/~api/projects/1/setting") => {
                *p.lock().unwrap() = r.body.clone();
                Reply::json("1")
            }
            _ => fake::not_found(),
        });
        let cfg = fake::config("onedev", &srv.url());
        let o = OneDev::new(&cfg);
        let repo = Repo {
            owner: ONEDEV_ROOT.into(),
            name: "p1".into(),
            ..Default::default()
        };
        assert_eq!(o.hooks().unwrap().repo(&repo, false).unwrap(), "created");
        let body = String::from_utf8(posted.lock().unwrap().clone()).unwrap();
        assert!(
            body.starts_with(r#"{"alpha":[1.50],"webHooks":[{"#),
            "{body}"
        );
        assert!(
            body.ends_with(r#""zeta":{"big":123456789012345678901234567890,"f":1e3,"neg":-0.0}}"#),
            "{body}"
        );
        assert!(body.contains(&format!(r#""postUrl":"{}""#, cfg.hook_url())));
    }

    #[test]
    fn webhook_payload_with_broken_surrogates_still_parses() {
        let srv = TestServer::start(|_| {
            fake::js(&json!({"id": 2, "name": "child", "path": "parent/child"}))
        });
        let o = OneDev::new(&fake::config("onedev", &srv.url()));
        let body = br#"{"projectId":2,"comment":"bad \ud83d here"}"#;
        assert!(serde_json::from_slice::<Value>(body).is_err());
        let h = fake::headers(&[("X-OneDev-Signature", "s3cret")]);
        fake::must_event(
            o.parse_webhook(&h, body, "s3cret"),
            Change::Changed,
            "parent",
            "child",
            2,
        );
    }

    #[test]
    fn project_paths_are_escaped_per_segment() {
        assert_eq!(escape_path("parent/my child"), "parent/my%20child");
        assert_eq!(project_path(ONEDEV_ROOT, "p"), "p");
        assert_eq!(project_path("a/b", "p"), "a/b/p");
    }
}
