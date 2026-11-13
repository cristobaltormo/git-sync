use serde::{Deserialize, Deserializer, Serialize, Serializer};

use std::collections::HashMap;
use std::sync::Mutex;

use serde_json::json;

use crate::config::Config;
use crate::error::Error;
use crate::forge::USER_AGENT;
use crate::forge::common::{decode, eq_fold, query_escape};
use crate::httpx::{self, Response, fail};

#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(default)]
pub struct Owner {
    #[serde(deserialize_with = "null_as_default")]
    pub login: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(default)]
pub struct Repo {
    #[serde(deserialize_with = "null_as_default")]
    pub name: String,
    #[serde(deserialize_with = "null_as_default")]
    pub owner: Owner,
    #[serde(deserialize_with = "null_as_default")]
    pub private: bool,
    #[serde(deserialize_with = "null_as_default")]
    pub archived: bool,
    pub description: Option<String>,
    pub homepage: Option<String>,
    #[serde(deserialize_with = "null_as_default")]
    pub default_branch: String,
    #[serde(deserialize_with = "null_as_default")]
    pub topics: Vec<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Snap {
    pub private: bool,
    pub archived: bool,
    #[serde(default, deserialize_with = "null_as_default")]
    pub description: String,
    #[serde(default, deserialize_with = "null_as_default")]
    pub homepage: String,
    #[serde(default, deserialize_with = "null_as_default")]
    pub default_branch: String,
    #[serde(
        default,
        serialize_with = "empty_as_null",
        deserialize_with = "null_as_default"
    )]
    pub topics: Vec<String>,
}

fn null_as_default<'de, D, T>(d: D) -> Result<T, D::Error>
where
    D: Deserializer<'de>,
    T: Default + Deserialize<'de>,
{
    Ok(Option::<T>::deserialize(d)?.unwrap_or_default())
}

fn empty_as_null<S: Serializer>(v: &[String], s: S) -> Result<S::Ok, S::Error> {
    if v.is_empty() {
        s.serialize_none()
    } else {
        v.serialize(s)
    }
}

fn text(s: &Option<String>) -> String {
    s.as_deref().map(str::trim).unwrap_or_default().to_string()
}

pub fn snapshot(r: &Repo) -> Snap {
    let mut topics = r.topics.clone();
    topics.sort();
    Snap {
        private: r.private,
        archived: r.archived,
        description: text(&r.description),
        homepage: text(&r.homepage),
        default_branch: r.default_branch.clone(),
        topics,
    }
}

pub fn clean_topics(topics: &[String], drop: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for t in topics {
        let lower = t.to_lowercase().replace('_', "-");
        let mut s = String::with_capacity(lower.len());
        let mut junk = false;
        for c in lower.chars() {
            if c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' {
                s.push(c);
                junk = false;
            } else if !junk {
                s.push('-');
                junk = true;
            }
        }
        let mut s = s.trim_matches('-').to_string();
        if s.len() > 50 {
            s.truncate(50);
        }
        if s.is_empty() || s == drop || out.contains(&s) {
            continue;
        }
        out.push(s);
    }
    out.truncate(20);
    out
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(default)]
pub struct Listed {
    #[serde(deserialize_with = "null_as_default")]
    pub name: String,
    #[serde(deserialize_with = "null_as_default")]
    pub full_name: String,
    #[serde(deserialize_with = "null_as_default")]
    pub private: bool,
    #[serde(deserialize_with = "null_as_default")]
    pub archived: bool,
    #[serde(deserialize_with = "null_as_default")]
    pub fork: bool,
    #[serde(deserialize_with = "null_as_default")]
    pub html_url: String,
    #[serde(deserialize_with = "null_as_default")]
    pub pushed_at: String,
    #[serde(deserialize_with = "null_as_default")]
    pub owner: Owner,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Pull {
    pub owner: String,
    pub repo: String,
    pub number: i64,
    pub title: String,
    pub url: String,
    pub author: String,
}

impl Pull {
    pub fn key(&self) -> String {
        format!("{}/{}#{}", self.owner, self.repo, self.number)
    }
}

pub type Fields = serde_json::Map<String, serde_json::Value>;

pub trait Target: Send + Sync {
    fn get(&self, owner: &str, name: &str) -> Result<Option<Repo>, Error>;
    fn create(
        &self,
        owner: &str,
        name: &str,
        description: &str,
        homepage: &str,
    ) -> Result<Option<Repo>, Error>;
    fn patch(&self, owner: &str, name: &str, fields: &Fields) -> Result<(), Error>;
    fn set_topics(&self, owner: &str, name: &str, topics: &[String]) -> Result<(), Error>;
    fn delete(&self, owner: &str, name: &str) -> Result<(), Error>;
    fn open_pulls(&self, owner: &str) -> Result<Vec<Pull>, Error>;
    fn comment(&self, owner: &str, name: &str, number: i64, body: &str) -> Result<(), Error>;
}

pub struct Client {
    dry: bool,
    base: String,
    http: httpx::Client,
    me: Mutex<Option<(String, Option<String>)>>,
    types: Mutex<HashMap<String, String>>,
}

impl Client {
    pub fn new(cfg: &Config, dry: bool) -> Client {
        let auth = format!("Bearer {}", cfg.github.token);
        Client {
            dry,
            base: cfg.github.api_url.trim_end_matches('/').to_string(),
            http: httpx::Client::new(&[
                ("Authorization", &auth),
                ("Accept", "application/vnd.github+json"),
                ("X-GitHub-Api-Version", "2022-11-28"),
                ("User-Agent", USER_AGENT),
            ]),
            me: Mutex::new(None),
            types: Mutex::new(HashMap::new()),
        }
    }

    #[cfg(test)]
    pub fn http_mut(&mut self) -> &mut httpx::Client {
        &mut self.http
    }

    pub fn whoami(&self) -> Result<(String, Option<String>), Error> {
        if let Some(me) = self.me.lock().unwrap_or_else(|e| e.into_inner()).as_ref() {
            return Ok(me.clone());
        }
        let r = self.http.get(&format!("{}/user", self.base))?;
        if r.status == 401 {
            return Err(Error::api(401, "the GitHub token was rejected"));
        }
        if !r.ok() {
            return Err(fail(&r, ""));
        }
        let u: Owner = decode(&r)?;
        let scope = r.headers.value("X-OAuth-Scopes").map(str::to_string);
        if !u.login.is_empty() {
            *self.me.lock().unwrap_or_else(|e| e.into_inner()) =
                Some((u.login.clone(), scope.clone()));
        }
        Ok((u.login, scope))
    }

    pub fn owner_type(&self, login: &str) -> Result<String, Error> {
        if let Some(t) = self
            .types
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(login)
        {
            return Ok(t.clone());
        }
        let r = self.http.get(&format!("{}/users/{login}", self.base))?;
        if !r.ok() {
            return Err(Error::api(
                r.status,
                format!("GitHub account '{login}' not found"),
            ));
        }
        #[derive(Deserialize, Default)]
        #[serde(default)]
        struct U {
            #[serde(rename = "type", deserialize_with = "null_as_default")]
            kind: String,
        }
        let u: U = decode(&r)?;
        self.types
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(login.to_string(), u.kind.clone());
        Ok(u.kind)
    }

    fn send<B: Serialize + ?Sized>(
        &self,
        method: &str,
        path: &str,
        body: Option<&B>,
    ) -> Result<Option<Response>, Error> {
        if self.dry {
            match body {
                Some(b) => crate::infof!(
                    "dry-run: {method} {path} {}",
                    serde_json::to_string(b).unwrap_or_default()
                ),
                None => crate::infof!("dry-run: {method} {path}"),
            }
            return Ok(None);
        }
        let url = format!("{}{path}", self.base);
        let r = match body {
            Some(b) => self.http.json(method, &url, b)?,
            None => self.http.request(method, &url, None)?,
        };
        if !r.ok() {
            return Err(fail(&r, ""));
        }
        Ok(Some(r))
    }

    pub fn list_repos(&self, owner: &str) -> Result<Vec<Listed>, Error> {
        let (me, _) = self.whoami()?;
        let base = if eq_fold(owner, &me) {
            "/user/repos?affiliation=owner".to_string()
        } else if self.owner_type(owner)? == "Organization" {
            format!("/orgs/{owner}/repos?type=all")
        } else {
            format!("/users/{owner}/repos")
        };
        let sep = if base.contains('?') { '&' } else { '?' };
        let mut out = Vec::new();
        for page in 1..=30 {
            let r = self
                .http
                .get(&format!("{}{base}{sep}per_page=100&page={page}", self.base))?;
            if !r.ok() {
                return Err(fail(&r, &format!("listing {owner}")));
            }
            let rows: Vec<Listed> = decode::<Option<_>>(&r)?.unwrap_or_default();
            let full = rows.len() >= 100;
            out.extend(rows.into_iter().filter(|x| eq_fold(&x.owner.login, owner)));
            if !full {
                break;
            }
        }
        Ok(out)
    }
}

impl Target for Client {
    fn get(&self, owner: &str, name: &str) -> Result<Option<Repo>, Error> {
        let r = self
            .http
            .get(&format!("{}/repos/{owner}/{name}", self.base))?;
        if r.status == 404 {
            return Ok(None);
        }
        if !r.ok() {
            return Err(fail(&r, ""));
        }
        let x: Repo = decode(&r)?;
        if !eq_fold(&x.name, name) || (!x.owner.login.is_empty() && !eq_fold(&x.owner.login, owner))
        {
            return Ok(None);
        }
        Ok(Some(x))
    }

    fn create(
        &self,
        owner: &str,
        name: &str,
        description: &str,
        homepage: &str,
    ) -> Result<Option<Repo>, Error> {
        let mut body =
            json!({"name": name, "private": true, "has_wiki": false, "has_projects": false});
        if !description.is_empty() {
            body["description"] = json!(description);
        }
        if !homepage.is_empty() {
            body["homepage"] = json!(homepage);
        }
        let (me, _) = self.whoami()?;
        let path = if eq_fold(owner, &me) {
            "/user/repos".to_string()
        } else {
            if self.owner_type(owner)? != "Organization" {
                return Err(Error::api(
                    403,
                    format!(
                        "the token belongs to '{me}' and cannot create repos under the user '{owner}'"
                    ),
                ));
            }
            format!("/orgs/{owner}/repos")
        };
        match self.send("POST", &path, Some(&body))? {
            Some(r) => Ok(Some(decode(&r)?)),
            None => Ok(None),
        }
    }

    fn patch(&self, owner: &str, name: &str, fields: &Fields) -> Result<(), Error> {
        self.send("PATCH", &format!("/repos/{owner}/{name}"), Some(fields))?;
        Ok(())
    }

    fn set_topics(&self, owner: &str, name: &str, topics: &[String]) -> Result<(), Error> {
        self.send(
            "PUT",
            &format!("/repos/{owner}/{name}/topics"),
            Some(&json!({"names": topics})),
        )?;
        Ok(())
    }

    fn delete(&self, owner: &str, name: &str) -> Result<(), Error> {
        self.send::<()>("DELETE", &format!("/repos/{owner}/{name}"), None)?;
        Ok(())
    }

    fn open_pulls(&self, owner: &str) -> Result<Vec<Pull>, Error> {
        #[derive(Deserialize, Default)]
        #[serde(default)]
        struct User {
            #[serde(deserialize_with = "null_as_default")]
            login: String,
        }
        #[derive(Deserialize, Default)]
        #[serde(default)]
        struct Item {
            #[serde(deserialize_with = "null_as_default")]
            number: i64,
            #[serde(deserialize_with = "null_as_default")]
            title: String,
            #[serde(deserialize_with = "null_as_default")]
            html_url: String,
            #[serde(deserialize_with = "null_as_default")]
            repository_url: String,
            #[serde(deserialize_with = "null_as_default")]
            user: User,
        }
        #[derive(Deserialize, Default)]
        #[serde(default)]
        struct Found {
            #[serde(deserialize_with = "null_as_default")]
            items: Vec<Item>,
        }
        let q = query_escape(&format!("is:pr is:open user:{owner}")).into_owned();
        let mut out = Vec::new();
        for page in 1..=10 {
            let r = self.http.get(&format!(
                "{}/search/issues?page={page}&per_page=100&q={q}",
                self.base
            ))?;
            if !r.ok() {
                return Err(fail(&r, &format!("searching pull requests of {owner}")));
            }
            let found: Found = decode(&r)?;
            let full = found.items.len() >= 100;
            for it in found.items {
                let parts: Vec<&str> = it.repository_url.trim_end_matches('/').split('/').collect();
                if parts.len() < 2 {
                    continue;
                }
                out.push(Pull {
                    owner: parts[parts.len() - 2].to_string(),
                    repo: parts[parts.len() - 1].to_string(),
                    number: it.number,
                    title: it.title,
                    url: it.html_url,
                    author: it.user.login,
                });
            }
            if !full {
                break;
            }
        }
        Ok(out)
    }

    fn comment(&self, owner: &str, name: &str, number: i64, body: &str) -> Result<(), Error> {
        self.send(
            "POST",
            &format!("/repos/{owner}/{name}/issues/{number}/comments"),
            Some(&json!({"body": body})),
        )?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snapshots_trim_and_sort() {
        let r: Repo = serde_json::from_str(
            r#"{"name":"x","owner":{"login":"o"},"private":true,"description":" d ","homepage":null,"default_branch":"main","topics":["b","a"],"extra":1}"#,
        )
        .unwrap();
        let s = snapshot(&r);
        assert_eq!(s.description, "d");
        assert_eq!(s.homepage, "");
        assert_eq!(s.topics, vec!["a", "b"]);
        assert!(s.private);
    }

    #[test]
    fn null_fields_decode_as_zero_values() {
        let r: Repo = serde_json::from_str(
            r#"{"name":null,"owner":null,"private":null,"archived":null,"description":null,"default_branch":null,"topics":null}"#,
        )
        .unwrap();
        assert_eq!(r, Repo::default());
        let r: Repo = serde_json::from_str(r#"{"name":"x","owner":{"login":null}}"#).unwrap();
        assert_eq!(r.owner.login, "");
        let l: Listed = serde_json::from_str(
            r#"{"name":null,"full_name":null,"private":null,"archived":null,"fork":null,"html_url":null,"pushed_at":null,"owner":null}"#,
        )
        .unwrap();
        assert_eq!(l, Listed::default());
        let rows: Option<Vec<Listed>> = serde_json::from_str("null").unwrap();
        assert!(rows.is_none());
    }

    #[test]
    fn snap_json_matches_go() {
        let s = Snap {
            private: true,
            default_branch: "main".into(),
            ..Default::default()
        };
        let js = serde_json::to_string(&s).unwrap();
        assert_eq!(
            js,
            r#"{"private":true,"archived":false,"description":"","homepage":"","default_branch":"main","topics":null}"#
        );
        let back: Snap = serde_json::from_str(&js).unwrap();
        assert_eq!(back, s);
        let with: Snap =
            serde_json::from_str(r#"{"private":false,"archived":false,"topics":["a"]}"#).unwrap();
        assert_eq!(
            serde_json::to_string(&with).unwrap(),
            r#"{"private":false,"archived":false,"description":"","homepage":"","default_branch":"","topics":["a"]}"#
        );
    }

    #[test]
    fn topics_are_cleaned_like_github_wants() {
        let input: Vec<String> = [
            "Rust_Lang",
            "  web  dev ",
            "rust-lang",
            "--x--",
            "",
            "drop-me",
            "Ünïcode",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect();
        assert_eq!(
            clean_topics(&input, "drop-me"),
            vec!["rust-lang", "web-dev", "x", "n-code"]
        );
        let many: Vec<String> = (0..30).map(|i| format!("t{i}")).collect();
        assert_eq!(clean_topics(&many, "").len(), 20);
        let long = vec!["a".repeat(80)];
        assert_eq!(clean_topics(&long, "")[0].len(), 50);
    }

    #[test]
    fn pull_keys() {
        let p = Pull {
            owner: "o".into(),
            repo: "r".into(),
            number: 7,
            ..Default::default()
        };
        assert_eq!(p.key(), "o/r#7");
    }
}

#[cfg(test)]
mod client_tests {
    use super::*;
    use crate::forge::common::fake::query;
    use crate::testutil::{Reply, TestServer};
    use crate::webhook::Request;

    const TOKEN: &str = "gh-token-123456";

    fn client(
        dry: bool,
        route: impl Fn(&Request) -> Reply + Send + Sync + 'static,
    ) -> (Client, TestServer) {
        crate::logx::silence();
        let srv = TestServer::start(move |r| {
            if r.header("authorization") != format!("Bearer {TOKEN}") {
                return Reply::status(401);
            }
            route(r)
        });
        let mut cfg = Config::default();
        cfg.github.token = TOKEN.into();
        cfg.github.api_url = format!("{}/", srv.url());
        let mut c = Client::new(&cfg, dry);
        c.http_mut().set_sleep(|_| {});
        (c, srv)
    }

    #[test]
    fn create_under_user_and_organization() {
        let (c, srv) = client(false, |r| match r.path() {
            "/user" => {
                Reply::json(r#"{"login":"alice-gh"}"#).header("X-OAuth-Scopes", "repo, delete_repo")
            }
            "/users/my-org" => Reply::json(r#"{"type":"Organization"}"#),
            "/users/bob" => Reply::json(r#"{"type":"User"}"#),
            _ => Reply {
                status: 201,
                ..Reply::json(r#"{"name":"site","private":true,"owner":{"login":"x"}}"#)
            },
        });
        assert!(c.create("alice-gh", "site", "desc", "").unwrap().is_some());
        assert!(
            c.create("my-org", "site", "", "https://x.example")
                .unwrap()
                .is_some()
        );
        let err = c.create("bob", "site", "", "").unwrap_err();
        assert!(
            err.to_string()
                .contains("cannot create repos under the user"),
            "{err}"
        );
        assert_eq!(err.status(), Some(403));
        let posts: Vec<_> = srv
            .requests()
            .into_iter()
            .filter(|s| s.method == "POST")
            .collect();
        let paths: Vec<&str> = posts.iter().map(|s| s.target.as_str()).collect();
        assert_eq!(paths, ["/user/repos", "/orgs/my-org/repos"]);
        for p in &posts {
            assert!(String::from_utf8_lossy(&p.body).contains(r#""private":true"#));
        }
        assert_eq!(
            String::from_utf8_lossy(&posts[0].body),
            r#"{"description":"desc","has_projects":false,"has_wiki":false,"name":"site","private":true}"#
        );
        let (login, scopes) = c.whoami().unwrap();
        assert_eq!(login, "alice-gh");
        assert!(scopes.unwrap().contains("delete_repo"));
        let users = srv.lines().iter().filter(|l| *l == "GET /user").count();
        assert_eq!(users, 1);
    }

    #[test]
    fn get() {
        let (c, _srv) = client(false, |r| match r.path() {
            "/repos/o/missing" => Reply::status(404),
            "/repos/o/old-name" => Reply::json(r#"{"name":"new-name","owner":{"login":"o"}}"#),
            "/repos/o/moved" => Reply::json(r#"{"name":"moved","owner":{"login":"someone-else"}}"#),
            "/repos/o/forbidden" => Reply::status(403).body(r#"{"message":"nope"}"#),
            "/repos/o/broken" => {
                Reply::json(r#"{"name":"broken","description":"x\ud83d","owner":{"login":"o"}}"#)
            }
            _ => Reply::json(
                r#"{"name":"site","description":null,"topics":["b","a"],"owner":{"login":"o"}}"#,
            ),
        });
        assert!(c.get("o", "missing").unwrap().is_none());
        assert!(c.get("o", "old-name").unwrap().is_none());
        assert!(c.get("o", "moved").unwrap().is_none());
        let err = c.get("o", "forbidden").unwrap_err();
        assert_eq!(err.status(), Some(403));
        assert!(err.to_string().contains("403"));
        let s = snapshot(&c.get("o", "site").unwrap().unwrap());
        assert_eq!(s.description, "");
        assert_eq!(s.topics, ["a", "b"]);
        let b = c.get("o", "broken").unwrap().unwrap();
        assert_eq!(b.description.as_deref(), Some("x\u{fffd}"));
    }

    #[test]
    fn errors_keep_status_and_message() {
        let (c, _srv) = client(false, |_| {
            Reply::status(422).body(r#"{"message":"Visibility change in progress"}"#)
        });
        let mut f = Fields::new();
        f.insert("private".into(), serde_json::Value::Bool(false));
        let err = c.patch("o", "r", &f).unwrap_err();
        match err {
            Error::Api { status, msg } => {
                assert_eq!(status, 422);
                assert!(msg.to_lowercase().contains("in progress"));
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn dry_run_never_mutates() {
        let (c, srv) = client(true, |_| Reply::json("{}"));
        let mut f = Fields::new();
        f.insert("private".into(), serde_json::Value::Bool(false));
        c.patch("o", "r", &f).unwrap();
        c.set_topics("o", "r", &["a".into()]).unwrap();
        c.delete("o", "r").unwrap();
        c.comment("o", "r", 1, "hi").unwrap();
        assert!(srv.requests().is_empty());
    }

    #[test]
    fn mutations_hit_the_right_paths() {
        let (c, srv) = client(false, |r| match r.method.as_str() {
            "DELETE" => Reply::status(204),
            _ => Reply::json("{}"),
        });
        c.set_topics("o", "r", &[]).unwrap();
        c.delete("o", "r").unwrap();
        c.comment("o", "r", 7, "hello").unwrap();
        let seen = srv.requests();
        assert_eq!(
            srv.lines(),
            [
                "PUT /repos/o/r/topics",
                "DELETE /repos/o/r",
                "POST /repos/o/r/issues/7/comments"
            ]
        );
        assert_eq!(seen[0].body, br#"{"names":[]}"#);
        assert!(seen[1].body.is_empty());
        assert_eq!(seen[2].body, br#"{"body":"hello"}"#);
    }

    #[test]
    fn open_pulls_follow_pages() {
        let (c, srv) = client(false, |r| {
            let page: usize = query(&r.target, "page").parse().unwrap();
            let n = if page == 1 { 100 } else { 1 };
            let items: Vec<String> = (0..n)
                .map(|i| {
                    format!(
                        r#"{{"number":{i},"title":"t","html_url":"u","repository_url":"https://api/repos/o/r{page}/","user":{{"login":"bot"}}}}"#
                    )
                })
                .collect();
            Reply::json(&format!(r#"{{"items":[{}]}}"#, items.join(",")))
        });
        let pulls = c.open_pulls("o").unwrap();
        assert_eq!(pulls.len(), 101);
        assert_eq!(pulls[100].key(), "o/r2#0");
        assert_eq!(pulls[0].author, "bot");
        assert_eq!(
            srv.lines()[0],
            "GET /search/issues?page=1&per_page=100&q=is%3Apr+is%3Aopen+user%3Ao"
        );
        assert_eq!(srv.requests().len(), 2);
    }

    #[test]
    fn list_repos_by_owner_kind() {
        let (c, srv) = client(false, |r| match r.path() {
            "/user" => Reply::json(r#"{"login":"me"}"#),
            "/users/org" => Reply::json(r#"{"type":"Organization"}"#),
            "/users/bob" => Reply::json(r#"{"type":"User"}"#),
            "/users/ghost" => Reply::status(404),
            _ => Reply::json(
                r#"[{"name":"a","full_name":"me/a","owner":{"login":"Me"},"pushed_at":null},
                    {"name":"b","full_name":"x/b","owner":{"login":"x"}}]"#,
            ),
        });
        assert_eq!(c.list_repos("me").unwrap().len(), 1);
        c.list_repos("org").unwrap();
        c.list_repos("bob").unwrap();
        let err = c.list_repos("ghost").unwrap_err();
        assert_eq!(
            err.to_string(),
            "HTTP 404: GitHub account 'ghost' not found"
        );
        let lines = srv.lines();
        assert!(lines.contains(&"GET /user/repos?affiliation=owner&per_page=100&page=1".into()));
        assert!(lines.contains(&"GET /orgs/org/repos?type=all&per_page=100&page=1".into()));
        assert!(lines.contains(&"GET /users/bob/repos?per_page=100&page=1".into()));
        c.list_repos("org").unwrap();
        let types = srv
            .lines()
            .iter()
            .filter(|l| *l == "GET /users/org")
            .count();
        assert_eq!(types, 1);
    }

    #[test]
    fn rejected_token() {
        let srv = TestServer::start(|_| Reply::status(401));
        let mut cfg = Config::default();
        cfg.github.api_url = srv.url();
        let err = Client::new(&cfg, false).whoami().unwrap_err();
        assert_eq!(err.to_string(), "HTTP 401: the GitHub token was rejected");
    }
}
