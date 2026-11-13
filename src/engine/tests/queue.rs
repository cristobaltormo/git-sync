use std::sync::Arc;
use std::time::{Duration, Instant};

use super::super::{HOOK_DELAY, Job, lock};
use super::{Harness, mk, nothing};
use crate::forge::{Change, Event};

#[test]
fn coalescing() {
    let h = Harness::new(nothing);
    let e = &h.e;
    let job = |why| Job {
        key: "1".into(),
        owner: "alice".into(),
        name: "site".into(),
        why,
        ..Job::default()
    };
    e.enqueue(job("a"));
    e.enqueue(job("b"));
    assert_eq!(e.queued(), 1);
    {
        let mut l = lock(&e.queue.lanes);
        let k = l.order.pop_front().unwrap();
        l.pending.remove(&k);
        l.running.insert(k);
    }
    e.enqueue(Job {
        key: "1".into(),
        why: "c",
        ..Job::default()
    });
    e.enqueue(Job {
        key: "1".into(),
        why: "d",
        verify: true,
        ..Job::default()
    });
    e.enqueue(Job {
        key: "1".into(),
        why: "e",
        ..Job::default()
    });
    let l = lock(&e.queue.lanes);
    assert_eq!(l.dirty.len(), 1);
    assert!(
        l.dirty["1"].verify,
        "a follow-up keeps the strongest request"
    );
    drop(l);
    assert_eq!(e.queued(), 0);
}

#[test]
fn workers_end_to_end() {
    let h = Harness::new(nothing);
    for i in 1..=6 {
        h.src.set(mk(i, &format!("r{i}")));
    }
    h.e.start();
    h.e.reconcile(false, "scan");
    assert!(h.e.drain(Duration::from_secs(10)), "queue did not drain");
    h.e.shutdown();
    assert_eq!(h.mir.count(), 6);
}

#[test]
fn webhook_burst_runs_once() {
    let h = Harness::new(nothing);
    h.src.set(mk(1, "site"));
    h.e.start();
    let start = Instant::now();
    let ev = Event {
        kind: Change::Changed,
        owner: "alice".into(),
        name: "site".into(),
        id: 1,
    };
    h.e.handle_event(&ev);
    h.e.handle_event(&ev);
    assert!(h.e.drain(Duration::from_secs(5)), "queue did not drain");
    h.e.shutdown();
    assert!(
        start.elapsed() >= HOOK_DELAY,
        "the job must wait for the debounce window"
    );
    assert_eq!(h.mir.count(), 1);
}

#[test]
fn a_panicking_job_is_recorded_and_the_worker_survives() {
    let h = Harness::new(nothing);
    h.src.set(mk(1, "site"));
    h.mir.panic.store(true, std::sync::atomic::Ordering::SeqCst);
    h.e.start();
    h.e.reconcile(false, "scan");
    assert!(h.e.drain(Duration::from_secs(5)));
    let x = h.entry(1).unwrap();
    assert_eq!(x.last_error, "internal error: boom");
    h.mir
        .panic
        .store(false, std::sync::atomic::Ordering::SeqCst);
    h.e.enqueue(Job::for_repo(Arc::new(mk(1, "site")), "sync", true, true));
    assert!(h.e.drain(Duration::from_secs(5)));
    h.e.shutdown();
    assert_eq!(h.mir.count(), 1);
    assert_eq!(h.entry(1).unwrap().last_error, "");
}

#[test]
fn shutdown_is_prompt_when_idle() {
    let h = Harness::new(nothing);
    h.e.start();
    let t = Instant::now();
    h.e.shutdown();
    assert!(t.elapsed() < Duration::from_secs(2));
}
