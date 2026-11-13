use std::sync::{Condvar, Mutex};
use std::time::{Duration, Instant};

use super::{Engine, lock, secs, unix_now};

#[derive(Default)]
pub(super) struct Wake {
    flags: Mutex<Flags>,
    cond: Condvar,
}

#[derive(Default)]
struct Flags {
    woken: bool,
    stopped: bool,
}

const HOOK_RETRY: Duration = Duration::from_secs(30);

impl Engine {
    pub fn wake(&self) {
        let mut f = lock(&self.wake.flags);
        f.woken = true;
        self.wake.cond.notify_all();
    }

    pub fn stop(&self) {
        let mut f = lock(&self.wake.flags);
        f.stopped = true;
        self.wake.cond.notify_all();
    }

    pub fn stopped(&self) -> bool {
        lock(&self.wake.flags).stopped
    }

    #[cfg(test)]
    pub(super) fn take_wake(&self) -> bool {
        std::mem::take(&mut lock(&self.wake.flags).woken)
    }

    fn sleep(&self, wait: Duration) -> Option<bool> {
        let f = lock(&self.wake.flags);
        let (mut f, _) = self
            .wake
            .cond
            .wait_timeout_while(f, wait, |f| !f.woken && !f.stopped)
            .unwrap_or_else(|e| e.into_inner());
        if f.stopped {
            return None;
        }
        Some(std::mem::take(&mut f.woken))
    }

    pub fn poll_loop(&self, mut on_tick: impl FnMut()) {
        let (mut first, mut asked, mut down) = (true, false, false);
        let mut last_fp: Option<u64> = None;
        let mut last_hook_try: Option<Instant> = None;
        let mut last_verify = Instant::now();
        let mut retry_seen: i64 = 0;
        while !self.stopped() {
            let now = Instant::now();
            on_tick();
            let cfg = self.config();
            let sy = &cfg.sync;
            if self.hook_mode().is_empty() && last_hook_try.is_none_or(|t| now - t >= HOOK_RETRY) {
                last_hook_try = Some(now);
                self.setup_hooks(false);
                if self.stopped() {
                    return;
                }
            }

            let listed = self.list_all();
            if self.stopped() {
                return;
            }
            match &listed {
                Err(e) => {
                    if !down {
                        warnf!("cannot reach the source, will keep trying quietly: {e}");
                    }
                    down = true;
                }
                Ok(repos) => {
                    let fp = fingerprint(repos);
                    let verify = first
                        || (sy.verify_interval > 0
                            && now - last_verify >= secs(sy.verify_interval as u64));
                    let why = if first {
                        "startup"
                    } else if down {
                        "source is back"
                    } else {
                        "change"
                    };
                    let retry_due = self
                        .next_retry(retry_seen)
                        .is_some_and(|at| at <= unix_now());
                    if first
                        || down
                        || asked
                        || verify
                        || last_fp != Some(fp)
                        || self.has_missing()
                        || retry_due
                    {
                        retry_seen = unix_now();
                        self.reconcile_repos(repos, verify, why);
                        last_fp = Some(fp);
                        if self.stopped() {
                            return;
                        }
                        if verify {
                            last_verify = now;
                            self.check_pulls();
                        }
                    }
                    first = false;
                    down = false;
                }
            }
            drop(listed);

            let mut wait = match sy.poll_interval {
                0 => Duration::from_secs(300),
                n => secs(n as u64),
            };
            if let Some(w) = self.missing_wait() {
                wait = wait.min(w);
            }
            if down {
                wait = wait.max(Duration::from_secs(15));
            }
            if let Some(at) = self.next_retry(retry_seen) {
                let until = Duration::from_secs((at - unix_now()).max(1) as u64);
                wait = wait.min(until);
            }
            match self.sleep(wait) {
                None => return,
                Some(w) => asked = w,
            }
        }
    }
}

pub(super) fn fingerprint(repos: &[std::sync::Arc<crate::forge::Repo>]) -> u64 {
    use std::hash::{DefaultHasher, Hash, Hasher};
    let mut sum: u64 = repos.len() as u64;
    for r in repos {
        let mut h = DefaultHasher::new();
        r.id.hash(&mut h);
        r.owner.hash(&mut h);
        r.name.hash(&mut h);
        r.private.hash(&mut h);
        r.description.hash(&mut h);
        r.website.hash(&mut h);
        r.archived.hash(&mut h);
        r.default_branch.hash(&mut h);
        let mut topics: u64 = 0;
        for t in &r.topics {
            let mut th = DefaultHasher::new();
            t.hash(&mut th);
            topics = topics.wrapping_add(th.finish());
        }
        topics.hash(&mut h);
        r.fork.hash(&mut h);
        r.updated.hash(&mut h);
        r.empty.hash(&mut h);
        r.mirror.hash(&mut h);
        r.role.hash(&mut h);
        sum = sum.wrapping_add(h.finish());
    }
    sum
}
