use std::collections::{HashMap, HashSet, VecDeque};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::{Arc, Condvar};
use std::thread;
use std::time::{Duration, Instant};

use super::{Engine, lock};
use crate::err;
use crate::forge::Repo;

#[derive(Debug, Clone, Default)]
pub struct Job {
    pub key: String,
    pub repo: Option<Arc<Repo>>,
    pub owner: String,
    pub name: String,
    pub why: &'static str,
    pub verify: bool,
    pub force: bool,
    pub ready_at: Option<Instant>,
}

impl Job {
    pub fn for_repo(repo: Arc<Repo>, why: &'static str, verify: bool, force: bool) -> Job {
        Job {
            key: repo.id.to_string(),
            repo: Some(repo),
            why,
            verify,
            force,
            ..Job::default()
        }
    }

    fn ready(&self, now: Instant) -> bool {
        self.ready_at.is_none_or(|at| at <= now)
    }
}

#[derive(Default)]
pub(super) struct Lanes {
    pub(super) pending: HashMap<String, Job>,
    pub(super) order: VecDeque<String>,
    pub(super) running: HashSet<String>,
    pub(super) dirty: HashMap<String, Job>,
    stopped: bool,
    alive: usize,
}

impl Lanes {
    fn busy(&self) -> bool {
        !self.pending.is_empty() || !self.running.is_empty()
    }
}

#[derive(Default)]
pub(super) struct Queue {
    pub(super) lanes: std::sync::Mutex<Lanes>,
    work: Condvar,
    idle: Condvar,
}

const WORKER_STACK: usize = 1 << 20;
const SHUTDOWN_WAIT: Duration = Duration::from_secs(60);

impl Engine {
    pub fn enqueue(&self, mut job: Job) {
        let q = &self.queue;
        let mut l = lock(&q.lanes);
        if l.running.contains(&job.key) {
            if let Some(old) = l.dirty.get(&job.key) {
                job.verify |= old.verify;
                job.force |= old.force;
            }
            l.dirty.insert(job.key.clone(), job);
            return;
        }
        if let Some(old) = l.pending.get_mut(&job.key) {
            old.verify |= job.verify;
            old.force |= job.force;
            match job.repo {
                Some(r) => old.repo = Some(r),
                None => {
                    old.repo = None;
                    if !job.owner.is_empty() {
                        old.owner = job.owner;
                        old.name = job.name;
                    }
                }
            }
            return;
        }
        l.order.push_back(job.key.clone());
        l.pending.insert(job.key.clone(), job);
        q.work.notify_one();
    }

    pub fn start(self: &Arc<Self>) {
        let n = self.config().sync.workers.max(1);
        for i in 0..n {
            let e = Arc::clone(self);
            lock(&self.queue.lanes).alive += 1;
            let spawned = thread::Builder::new()
                .name(format!("worker-{i}"))
                .stack_size(WORKER_STACK)
                .spawn(move || e.worker());
            if let Err(err) = spawned {
                lock(&self.queue.lanes).alive -= 1;
                errorf!("cannot start a worker: {err}");
            }
        }
    }

    fn next_job(&self) -> Option<Job> {
        let q = &self.queue;
        let mut l = lock(&q.lanes);
        loop {
            if l.stopped {
                return None;
            }
            let now = Instant::now();
            let mut soonest: Option<Duration> = None;
            let mut found = None;
            for (i, k) in l.order.iter().enumerate() {
                let j = &l.pending[k];
                if j.ready(now) {
                    found = Some(i);
                    break;
                }
                if let Some(at) = j.ready_at {
                    let d = at - now;
                    soonest = Some(soonest.map_or(d, |s| s.min(d)));
                }
            }
            if let Some(i) = found {
                let k = l.order.remove(i).unwrap_or_default();
                let job = l.pending.remove(&k).unwrap_or_default();
                l.running.insert(k);
                return Some(job);
            }
            l = match soonest {
                Some(d) => {
                    q.work
                        .wait_timeout(l, d)
                        .unwrap_or_else(|e| e.into_inner())
                        .0
                }
                None => q.work.wait(l).unwrap_or_else(|e| e.into_inner()),
            };
        }
    }

    fn worker(&self) {
        loop {
            match catch_unwind(AssertUnwindSafe(|| self.work())) {
                Ok(()) => break,
                Err(p) => errorf!("worker crashed ({}), restarting it", panic_text(&*p)),
            }
        }
        let mut l = lock(&self.queue.lanes);
        l.alive -= 1;
        self.queue.idle.notify_all();
    }

    fn work(&self) {
        while let Some(job) = self.next_job() {
            let _done = Done {
                e: self,
                key: &job.key,
            };
            self.execute(&job);
        }
    }

    fn finish_job(&self, key: &str) {
        let q = &self.queue;
        let mut l = lock(&q.lanes);
        l.running.remove(key);
        if let Some(again) = l.dirty.remove(key) {
            l.order.push_back(key.to_string());
            l.pending.insert(key.to_string(), again);
            q.work.notify_one();
        }
        if !l.busy() {
            q.idle.notify_all();
        }
    }

    pub(super) fn execute(&self, job: &Job) {
        match catch_unwind(AssertUnwindSafe(|| self.run_job(job))) {
            Ok(Ok(())) => {}
            Ok(Err(e)) => self.record_failure(job, &e),
            Err(p) => {
                let what = panic_text(&*p);
                drop(p);
                self.record_failure(job, &err!("internal error: {what}"));
            }
        }
    }

    pub fn drain(&self, timeout: Duration) -> bool {
        let q = &self.queue;
        let end = Instant::now() + timeout;
        let mut l = lock(&q.lanes);
        loop {
            if !l.busy() {
                return true;
            }
            let wait = if timeout.is_zero() {
                Duration::from_secs(1)
            } else {
                let now = Instant::now();
                if now >= end {
                    return false;
                }
                (end - now).min(Duration::from_secs(1))
            };
            l = q
                .idle
                .wait_timeout(l, wait)
                .unwrap_or_else(|e| e.into_inner())
                .0;
        }
    }

    pub fn shutdown(&self) {
        self.stop();
        let q = &self.queue;
        let end = Instant::now() + SHUTDOWN_WAIT;
        let mut l = lock(&q.lanes);
        l.stopped = true;
        q.work.notify_all();
        while l.alive > 0 {
            let now = Instant::now();
            if now >= end {
                warnf!("some jobs were still running at shutdown");
                break;
            }
            l = q
                .idle
                .wait_timeout(l, end - now)
                .unwrap_or_else(|e| e.into_inner())
                .0;
        }
        drop(l);
        self.state.save();
    }

    pub fn queued(&self) -> usize {
        lock(&self.queue.lanes).order.len()
    }
}

struct Done<'a> {
    e: &'a Engine,
    key: &'a str,
}

impl Drop for Done<'_> {
    fn drop(&mut self) {
        self.e.finish_job(self.key);
    }
}

fn panic_text(p: &(dyn std::any::Any + Send)) -> String {
    p.downcast_ref::<&str>()
        .map(|s| s.to_string())
        .or_else(|| p.downcast_ref::<String>().cloned())
        .unwrap_or_else(|| "panic".to_string())
}
