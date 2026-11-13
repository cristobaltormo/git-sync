use std::sync::atomic::Ordering;

use super::super::Job;
use super::{Harness, g, mk, nothing};
use crate::forge::{Change, Event};

fn ev(kind: Change, owner: &str, name: &str, id: i64) -> Event {
    Event {
        kind,
        owner: owner.into(),
        name: name.into(),
        id,
    }
}

#[test]
fn webhook_routing() {
    let h = Harness::new(nothing);
    h.e.handle_event(&ev(Change::Changed, "stranger", "x", 7));
    assert_eq!(h.e.queued(), 0);
    h.e.handle_event(&ev(Change::Changed, "alice", "site", 1));
    assert_eq!(h.e.queued(), 1);
    {
        let l = super::super::lock(&h.e.queue.lanes);
        let job = &l.pending["1"];
        assert!(
            job.repo.is_none() && job.owner == "alice" && job.name == "site",
            "webhook jobs fetch fresh data"
        );
    }
    h.e.handle_event(&ev(Change::Removed, "alice", "gone", 2));
    assert!(h.e.take_wake(), "a removed repo should wake the poller");
}

#[test]
fn webhook_for_missing_repo_wakes_poller() {
    let h = Harness::new(nothing);
    h.e.enqueue(Job {
        key: "5".into(),
        owner: "alice".into(),
        name: "ghost".into(),
        why: "webhook",
        ..Job::default()
    });
    h.pump();
    assert!(h.e.take_wake(), "expected a wake-up");
}

#[test]
fn hook_setup() {
    let h = Harness::new(nothing);
    h.src.hooks_on.store(true, Ordering::SeqCst);
    h.src.admin.store(true, Ordering::SeqCst);
    assert!(h.e.setup_hooks(false), "setup");
    assert_eq!(h.e.hook_mode(), "system");
    assert_eq!(h.e.system_hook(), 42);

    let h2 = Harness::new(nothing);
    h2.src.hooks_on.store(true, Ordering::SeqCst);
    h2.e.setup_hooks(false);
    assert_eq!(h2.e.hook_mode(), "repo");
    h2.src.set(mk(1, "site"));
    h2.run(false);
    assert_eq!(*g(&h2.src.hooks.repo), vec!["alice/site".to_string()]);
    h2.run(true);
    assert_eq!(g(&h2.src.hooks.repo).len(), 1, "the hook is created once");

    let h3 = Harness::new(nothing);
    h3.e.setup_hooks(false);
    assert_eq!(h3.e.hook_mode(), "none");

    let h4 = Harness::new(|c| c.hooks.mode = "none".into());
    h4.src.hooks_on.store(true, Ordering::SeqCst);
    h4.e.setup_hooks(false);
    assert_eq!(h4.e.hook_mode(), "none");
}
