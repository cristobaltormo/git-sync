mod filter;
mod hooks;
mod jobs;
mod notify;
mod poll;
mod pulls;
mod queue;
mod reconcile;
mod removal;
mod rename;
mod settings;
mod state;
mod sync;

#[cfg(test)]
mod tests;

use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex, MutexGuard, RwLock};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use crate::config::Config;
use crate::error::Error;
use crate::forge::{ONEDEV_ROOT, Repo};
use crate::mirror::{Options, Outcome};

pub use crate::forge::Source;
pub use crate::github::Target;

pub use filter::{Verdict, glob_match};
pub use hooks::HOOK_DELAY;
pub use notify::{EVENT_FAILING, EVENT_PENDING, EVENT_PULL_REQUEST, Event, send_test};
pub use queue::Job;
pub use state::{Entry, State, read_state};

pub trait Mirror: Send + Sync {
    fn mirror(
        &self,
        r: &Repo,
        gh_owner: &str,
        gh_name: &str,
        last: &str,
        opt: Options,
    ) -> Result<Outcome, Error>;
    fn forget(&self, id: i64);
}

pub struct Engine {
    cfg: RwLock<Arc<Config>>,
    src: Arc<dyn Source>,
    tgt: Arc<dyn Target>,
    mir: Arc<dyn Mirror>,
    dry: bool,
    queue: queue::Queue,
    state: state::Store,
    hook_mode: Mutex<&'static str>,
    wake: poll::Wake,
    deletions: Mutex<VecDeque<Instant>>,
    missing: Mutex<HashMap<i64, removal::Backoff>>,
    limit_logged: Mutex<Option<Instant>>,
}

impl Engine {
    pub fn new(
        cfg: Arc<Config>,
        src: Arc<dyn Source>,
        tgt: Arc<dyn Target>,
        mir: Arc<dyn Mirror>,
    ) -> Result<Arc<Engine>, Error> {
        let dry = cfg.sync.dry_run;
        let state = state::Store::load(&cfg.paths.state_dir, dry)?;
        Ok(Arc::new(Engine {
            cfg: RwLock::new(cfg),
            src,
            tgt,
            mir,
            dry,
            queue: queue::Queue::default(),
            state,
            hook_mode: Mutex::new(""),
            wake: poll::Wake::default(),
            deletions: Mutex::new(VecDeque::new()),
            missing: Mutex::new(HashMap::new()),
            limit_logged: Mutex::new(None),
        }))
    }

    pub fn config(&self) -> Arc<Config> {
        self.cfg.read().unwrap_or_else(|e| e.into_inner()).clone()
    }

    pub fn dry(&self) -> bool {
        self.dry
    }

    pub fn reload(&self, mut c: Config) {
        c.paths.state_dir = self.config().paths.state_dir.clone();
        *self.cfg.write().unwrap_or_else(|e| e.into_inner()) = Arc::new(c);
        self.state.clear_retries();
        self.wake();
    }

    pub fn entries(&self) -> HashMap<i64, Entry> {
        self.state.with(|st| st.repos.clone())
    }

    pub fn system_hook(&self) -> i64 {
        self.state.with(|st| st.system_hook)
    }

    pub fn set_system_hook(&self, id: i64) {
        self.state.set_system_hook(id);
    }
}

pub fn full_name(owner: &str, name: &str) -> String {
    if owner == ONEDEV_ROOT {
        return name.to_string();
    }
    format!("{owner}/{name}")
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

fn unix_now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

fn unix_now_f64() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs_f64())
        .unwrap_or(0.0)
}

fn eq_fold(a: &str, b: &str) -> bool {
    if a.is_ascii() && b.is_ascii() {
        return a.eq_ignore_ascii_case(b);
    }
    a.chars()
        .flat_map(char::to_lowercase)
        .eq(b.chars().flat_map(char::to_lowercase))
}

fn secs(n: u64) -> Duration {
    Duration::from_secs(n)
}
