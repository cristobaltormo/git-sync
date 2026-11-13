mod extra;
mod filter;
mod fixes;
mod hooks;
mod jobs;
mod policy;
mod pulls;
mod queue;
mod reconcile;
mod removal;
mod rename;
mod state;
mod sync;

use std::collections::{BTreeMap, HashMap};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};

use super::settings::join_fields;
use super::{Engine, Mirror};
use crate::config::{Config, RepoRule, Repos};
use crate::error::Error;
use crate::forge::{Hooks, Identity, Repo, Source};
use crate::github::{self, Fields, Pull, Target};
use crate::mirror::{Options, Outcome};
use crate::testutil::TempDir;

pub(super) type Log = Arc<Mutex<Vec<String>>>;

pub(super) fn g<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap()
}

#[derive(Default)]
pub(super) struct FakeHooks {
    pub global: Mutex<Vec<String>>,
    pub repo: Mutex<Vec<String>>,
}

impl Hooks for FakeHooks {
    fn global(&self, _known: i64, _force: bool) -> Result<(i64, String), Error> {
        g(&self.global).push("global".into());
        Ok((42, "created".into()))
    }

    fn repo(&self, r: &Repo, _force: bool) -> Result<String, Error> {
        g(&self.repo).push(r.full());
        Ok("created".into())
    }

    fn remove_global(&self, _id: i64) -> bool {
        true
    }

    fn remove_repo(&self, _r: &Repo) -> i64 {
        1
    }
}

pub(super) struct FakeSource {
    pub repos: Mutex<HashMap<i64, Repo>>,
    pub owner_ok: AtomicBool,
    pub owner_calls: AtomicUsize,
    pub list_err: Mutex<Option<String>>,
    pub hooks_on: AtomicBool,
    pub hooks: FakeHooks,
    pub admin: AtomicBool,
    pub hide: Mutex<Option<i64>>,
}

impl FakeSource {
    fn new() -> FakeSource {
        FakeSource {
            repos: Mutex::new(HashMap::new()),
            owner_ok: AtomicBool::new(true),
            owner_calls: AtomicUsize::new(0),
            list_err: Mutex::new(None),
            hooks_on: AtomicBool::new(false),
            hooks: FakeHooks::default(),
            admin: AtomicBool::new(false),
            hide: Mutex::new(None),
        }
    }

    pub fn set(&self, r: Repo) {
        g(&self.repos).insert(r.id, r);
    }

    pub fn remove(&self, id: i64) {
        g(&self.repos).remove(&id);
    }

    pub fn clear(&self) {
        g(&self.repos).clear();
    }
}

impl Source for FakeSource {
    fn kind(&self) -> &'static str {
        "fake"
    }

    fn whoami(&self) -> Result<Identity, Error> {
        let admin = self.admin.load(Ordering::SeqCst);
        Ok(Identity {
            login: "alice".into(),
            admin,
            system_hooks: admin,
        })
    }

    fn list(&self, owner: &str) -> Result<Vec<Arc<Repo>>, Error> {
        if let Some(e) = g(&self.list_err).clone() {
            return Err(Error::Other(e));
        }
        let hide = *g(&self.hide);
        Ok(g(&self.repos)
            .values()
            .filter(|r| r.owner == owner && Some(r.id) != hide)
            .map(|r| Arc::new(r.clone()))
            .collect())
    }

    fn get(&self, owner: &str, name: &str) -> Result<Option<Arc<Repo>>, Error> {
        Ok(g(&self.repos)
            .values()
            .find(|r| r.owner == owner && r.name == name)
            .map(|r| Arc::new(r.clone())))
    }

    fn owner_exists(&self, _owner: &str) -> Result<bool, Error> {
        self.owner_calls.fetch_add(1, Ordering::SeqCst);
        Ok(self.owner_ok.load(Ordering::SeqCst))
    }

    fn hooks(&self) -> Option<&dyn Hooks> {
        if self.hooks_on.load(Ordering::SeqCst) {
            Some(&self.hooks)
        } else {
            None
        }
    }
}

pub(super) struct FakeTarget {
    pub refuse_branch: AtomicBool,
    pub repos: Mutex<HashMap<String, github::Repo>>,
    pub calls: Mutex<Vec<String>>,
    pub pulls: Mutex<HashMap<String, Vec<Pull>>>,
    pub comments: Mutex<Vec<String>>,
    pub gets: AtomicUsize,
    events: Log,
}

impl FakeTarget {
    pub fn has(&self, key: &str) -> bool {
        g(&self.repos).contains_key(key)
    }

    pub fn repo(&self, key: &str) -> github::Repo {
        g(&self.repos).get(key).cloned().expect(key)
    }

    pub fn calls(&self) -> Vec<String> {
        g(&self.calls).clone()
    }
}

impl Target for FakeTarget {
    fn get(&self, owner: &str, name: &str) -> Result<Option<github::Repo>, Error> {
        self.gets.fetch_add(1, Ordering::SeqCst);
        Ok(g(&self.repos).get(&format!("{owner}/{name}")).cloned())
    }

    fn create(
        &self,
        owner: &str,
        name: &str,
        description: &str,
        homepage: &str,
    ) -> Result<Option<github::Repo>, Error> {
        g(&self.calls).push(format!("create {owner}/{name}"));
        g(&self.events).push("create".into());
        let r = github::Repo {
            name: name.into(),
            owner: github::Owner {
                login: owner.into(),
            },
            private: true,
            description: Some(description.into()),
            homepage: Some(homepage.into()),
            default_branch: "main".into(),
            ..github::Repo::default()
        };
        g(&self.repos).insert(format!("{owner}/{name}"), r.clone());
        Ok(Some(r))
    }

    fn patch(&self, owner: &str, name: &str, f: &Fields) -> Result<(), Error> {
        let desc = join_fields(f);
        g(&self.calls).push(format!("patch {owner}/{name} {desc}"));
        if f.contains_key("default_branch") && self.refuse_branch.load(Ordering::SeqCst) {
            return Err(Error::api(
                422,
                r#"{"message":"Validation Failed","errors":[{"resource":"Repository","code":"invalid","field":"default_branch"}]}"#,
            ));
        }
        g(&self.events).push(format!("patch {desc}"));
        let mut repos = g(&self.repos);
        let key = format!("{owner}/{name}");
        let mut r = repos.remove(&key).expect(&key);
        let s = |k: &str| f.get(k).and_then(|v| v.as_str()).map(str::to_string);
        let b = |k: &str| f.get(k).and_then(|v| v.as_bool());
        let mut key = key;
        if let Some(n) = s("name") {
            key = format!("{owner}/{n}");
            r.name = n;
        }
        if let Some(v) = b("private") {
            r.private = v;
        }
        if let Some(v) = b("archived") {
            r.archived = v;
        }
        if let Some(v) = s("description") {
            r.description = Some(v);
        }
        if let Some(v) = s("homepage") {
            r.homepage = Some(v);
        }
        if let Some(v) = s("default_branch") {
            r.default_branch = v;
        }
        repos.insert(key, r);
        Ok(())
    }

    fn set_topics(&self, owner: &str, name: &str, topics: &[String]) -> Result<(), Error> {
        g(&self.calls).push(format!("topics {owner}/{name}"));
        if let Some(r) = g(&self.repos).get_mut(&format!("{owner}/{name}")) {
            r.topics = topics.to_vec();
        }
        Ok(())
    }

    fn delete(&self, owner: &str, name: &str) -> Result<(), Error> {
        g(&self.calls).push(format!("delete {owner}/{name}"));
        g(&self.repos).remove(&format!("{owner}/{name}"));
        Ok(())
    }

    fn open_pulls(&self, owner: &str) -> Result<Vec<Pull>, Error> {
        Ok(g(&self.pulls).get(owner).cloned().unwrap_or_default())
    }

    fn comment(&self, owner: &str, name: &str, number: i64, body: &str) -> Result<(), Error> {
        g(&self.comments).push(format!("{owner}/{name}#{number}: {body}"));
        Ok(())
    }
}

#[derive(Default)]
pub(super) struct FakeMirror {
    pub calls: Mutex<Vec<String>>,
    pub lasts: Mutex<Vec<String>>,
    pub opts: Mutex<Vec<Options>>,
    pub fail: Mutex<Option<String>>,
    pub vanish: Mutex<Option<Arc<FakeSource>>>,
    pub panic: AtomicBool,
    events: Log,
}

impl FakeMirror {
    pub fn calls(&self) -> Vec<String> {
        g(&self.calls).clone()
    }

    pub fn count(&self) -> usize {
        g(&self.calls).len()
    }

    pub fn reset(&self) {
        g(&self.calls).clear();
    }

    pub fn last(&self) -> String {
        g(&self.lasts).last().cloned().unwrap_or_default()
    }
}

impl Mirror for FakeMirror {
    fn mirror(
        &self,
        r: &Repo,
        gh_owner: &str,
        gh_name: &str,
        last: &str,
        opt: Options,
    ) -> Result<Outcome, Error> {
        if let Some(src) = g(&self.vanish).as_ref() {
            src.remove(r.id);
            return Err(Error::other("git fetch failed: 404"));
        }
        if self.panic.load(Ordering::SeqCst) {
            panic!("boom");
        }
        if let Some(e) = g(&self.fail).clone() {
            return Err(Error::Other(e));
        }
        g(&self.calls).push(format!("mirror {} {gh_owner}/{gh_name}", r.full()));
        g(&self.lasts).push(last.to_string());
        g(&self.opts).push(opt);
        g(&self.events).push("mirror".into());
        Ok(Outcome {
            summary: "1 ref(s) updated".into(),
            refs: format!("refs-{}", r.updated),
        })
    }

    fn forget(&self, id: i64) {
        g(&self.calls).push(format!("forget {id}"));
    }
}

pub(super) struct Harness {
    pub dir: TempDir,
    pub src: Arc<FakeSource>,
    pub tgt: Arc<FakeTarget>,
    pub mir: Arc<FakeMirror>,
    pub events: Log,
    pub e: Arc<Engine>,
}

pub(super) fn test_config(dir: &str, f: &dyn Fn(&mut Config)) -> Config {
    let mut c = Config::default();
    c.source.url = "https://git.example.com".into();
    c.source.token = "src-token-123456".into();
    c.github.token = "gh-token-123456".into();
    c.accounts = BTreeMap::from([
        ("alice".to_string(), "alice-gh".to_string()),
        ("my-org".to_string(), "my-org-gh".to_string()),
    ]);
    c.listen.secret = "s3cret-value-123".into();
    c.paths.state_dir = dir.to_string();
    c.sync.delete_grace = 0;
    c.log.level = "off".into();
    f(&mut c);
    c.finish().unwrap();
    c
}

pub(super) fn nothing(_: &mut Config) {}

pub(super) fn with_rules(rules: &[(&str, RepoRule)]) -> impl Fn(&mut Config) + use<> {
    let rules: BTreeMap<String, RepoRule> = rules
        .iter()
        .map(|(k, v)| (k.to_lowercase(), v.clone()))
        .collect();
    move |c: &mut Config| {
        c.repos = Repos {
            path: String::new(),
            rules: rules.clone(),
        }
    }
}

impl Harness {
    pub fn new(f: impl Fn(&mut Config)) -> Harness {
        crate::logx::silence();
        let events: Log = Arc::default();
        let src = Arc::new(FakeSource::new());
        let tgt = Arc::new(FakeTarget {
            refuse_branch: AtomicBool::new(false),
            repos: Mutex::default(),
            calls: Mutex::default(),
            pulls: Mutex::default(),
            comments: Mutex::default(),
            gets: AtomicUsize::new(0),
            events: events.clone(),
        });
        let mir = Arc::new(FakeMirror {
            events: events.clone(),
            ..FakeMirror::default()
        });
        let dir = TempDir::new();
        let e = build(&dir, &src, &tgt, &mir, &f);
        Harness {
            dir,
            src,
            tgt,
            mir,
            events,
            e,
        }
    }

    pub fn rebuild(&mut self, f: impl Fn(&mut Config)) {
        self.e = build(&self.dir, &self.src, &self.tgt, &self.mir, &f);
    }

    pub fn pump(&self) {
        loop {
            let job = {
                let mut l = super::lock(&self.e.queue.lanes);
                let Some(k) = l.order.pop_front() else {
                    return;
                };
                l.pending.remove(&k).unwrap()
            };
            self.e.execute(&job);
        }
    }

    pub fn run(&self, verify: bool) {
        self.e.reconcile(verify, "scan");
        self.pump();
    }

    pub fn entry(&self, id: i64) -> Option<super::Entry> {
        self.e.state.get(id)
    }

    pub fn events(&self) -> Vec<String> {
        g(&self.events).clone()
    }
}

fn build(
    dir: &TempDir,
    src: &Arc<FakeSource>,
    tgt: &Arc<FakeTarget>,
    mir: &Arc<FakeMirror>,
    f: &dyn Fn(&mut Config),
) -> Arc<Engine> {
    let cfg = test_config(dir.path().to_str().unwrap(), f);
    Engine::new(Arc::new(cfg), src.clone(), tgt.clone(), mir.clone()).unwrap()
}

pub(super) fn mk(id: i64, name: &str) -> Repo {
    Repo {
        id,
        name: name.into(),
        owner: "alice".into(),
        private: true,
        default_branch: "main".into(),
        updated: "2026-01-01T00:00:00Z".into(),
        ..Repo::default()
    }
}

pub(super) fn upd(mut r: Repo, ts: &str) -> Repo {
    r.updated = ts.into();
    r
}

pub(super) fn synced(f: impl Fn(&mut Config)) -> Harness {
    let h = Harness::new(f);
    h.src.set(mk(1, "site"));
    h.run(false);
    g(&h.tgt.calls).clear();
    h
}
