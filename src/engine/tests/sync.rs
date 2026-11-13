use std::sync::Arc;

use super::super::{Job, unix_now};
use super::{Harness, g, mk, nothing, synced, upd};
use crate::forge::Repo;
use crate::github;

#[test]
fn new_private_repo() {
    let h = Harness::new(nothing);
    h.src.set(Repo {
        description: "my site".into(),
        ..mk(1, "site")
    });
    h.run(false);
    assert_eq!(h.tgt.calls()[0], "create alice-gh/site");
    assert_eq!(h.mir.calls(), vec!["mirror alice/site alice-gh/site"]);
    assert!(h.tgt.repo("alice-gh/site").private, "should be private");
    let x = h.entry(1).unwrap();
    assert!(x.managed && x.gh_name == "site", "entry not managed");
    assert_eq!(x.last_error, "");
}

#[test]
fn public_opens_only_after_push() {
    let h = Harness::new(nothing);
    h.src.set(Repo {
        private: false,
        description: "d".into(),
        topics: vec!["Python".into()],
        ..mk(1, "site")
    });
    h.run(false);
    let ev = h.events();
    assert_eq!(ev[0], "create");
    assert_eq!(ev[1], "mirror");
    assert_eq!(ev[2], "patch private=false");
    let r = h.tgt.repo("alice-gh/site");
    assert!(!r.private, "should be public");
    assert_eq!(r.topics, vec!["python".to_string()]);
}

#[test]
fn no_changes_no_calls() {
    let h = Harness::new(nothing);
    h.src.set(mk(1, "site"));
    h.run(false);
    let n = h.tgt.calls().len();
    h.e.reconcile(false, "scan");
    assert_eq!(h.e.queued(), 0);
    assert_eq!(h.tgt.calls().len(), n);
}

#[test]
fn visibility_both_ways() {
    let h = Harness::new(nothing);
    h.src.set(mk(1, "site"));
    h.run(false);
    h.src.set(Repo {
        private: false,
        ..upd(mk(1, "site"), "2026-02-01T00:00:00Z")
    });
    h.run(false);
    assert!(!h.tgt.repo("alice-gh/site").private, "should have opened");
    g(&h.events).clear();
    h.src.set(upd(mk(1, "site"), "2026-03-01T00:00:00Z"));
    h.run(false);
    assert!(h.tgt.repo("alice-gh/site").private, "should have closed");
    assert_eq!(h.events()[0], "patch private=true");
}

#[test]
fn visibility_off_leaves_github_alone() {
    let h = Harness::new(|c| c.sync.visibility = false);
    h.src.set(Repo {
        private: false,
        ..mk(1, "site")
    });
    h.run(false);
    assert!(h.tgt.repo("alice-gh/site").private, "must stay private");
}

#[test]
fn metadata_and_default_branch() {
    let h = Harness::new(nothing);
    h.src.set(mk(1, "site"));
    h.run(false);
    h.src.set(Repo {
        description: "new text".into(),
        website: "https://x.example".into(),
        default_branch: "trunk".into(),
        topics: vec!["a".into(), "b".into()],
        ..upd(mk(1, "site"), "2026-02-01T00:00:00Z")
    });
    h.run(false);
    let r = h.tgt.repo("alice-gh/site");
    assert_eq!(r.description.as_deref(), Some("new text"));
    assert_eq!(r.homepage.as_deref(), Some("https://x.example"));
    assert_eq!(r.default_branch, "trunk");
    assert_eq!(r.topics, vec!["a".to_string(), "b".to_string()]);
}

#[test]
fn metadata_off() {
    let h = Harness::new(|c| c.sync.metadata = false);
    h.src.set(Repo {
        description: "x".into(),
        topics: vec!["a".into()],
        ..mk(1, "site")
    });
    h.run(false);
    let r = h.tgt.repo("alice-gh/site");
    assert_eq!(r.description.as_deref(), Some(""));
    assert!(r.topics.is_empty());
}

#[test]
fn archive_flow() {
    let h = Harness::new(nothing);
    h.src.set(mk(1, "site"));
    h.run(false);
    h.src.set(Repo {
        archived: true,
        ..upd(mk(1, "site"), "2026-02-01T00:00:00Z")
    });
    h.run(false);
    assert!(h.tgt.repo("alice-gh/site").archived, "should be archived");
    g(&h.tgt.calls).clear();
    h.mir.reset();
    h.run(true);
    assert_eq!(h.mir.count(), 0);
    assert_eq!(h.tgt.calls().len(), 0);
    h.src.set(upd(mk(1, "site"), "2026-03-01T00:00:00Z"));
    h.run(false);
    assert!(
        !h.tgt.repo("alice-gh/site").archived,
        "should be unarchived"
    );
}

#[test]
fn existing_repo_needs_adoption() {
    let mut h = Harness::new(nothing);
    g(&h.tgt.repos).insert(
        "alice-gh/site".into(),
        github::Repo {
            name: "site".into(),
            private: true,
            default_branch: "main".into(),
            ..github::Repo::default()
        },
    );
    h.src.set(mk(1, "site"));
    h.run(false);
    assert_eq!(h.mir.count(), 0);
    let x = h.entry(1).unwrap();
    assert!(
        x.last_error.contains("adopt_existing") && !x.managed,
        "should be blocked"
    );
    h.rebuild(|c| c.sync.adopt_existing = true);
    h.e.state.update(1, false, |x| x.retry_at = 0);
    h.run(false);
    assert_eq!(h.mir.count(), 1);
    assert!(h.entry(1).unwrap().managed, "should be managed now");
}

#[test]
fn unmapped_owner_ignored() {
    let h = Harness::new(nothing);
    h.src.set(Repo {
        owner: "stranger".into(),
        ..mk(1, "x")
    });
    h.run(false);
    assert!(h.tgt.calls().is_empty());
}

#[test]
fn verify_skips_push_when_nothing_changed() {
    let h = synced(nothing);
    h.mir.reset();
    h.run(true);
    assert_eq!(h.mir.count(), 0);
    h.src.set(upd(mk(1, "site"), "2026-05-01T00:00:00Z"));
    h.run(true);
    assert_eq!(h.mir.count(), 1);
    h.mir.reset();
    h.e.state
        .update(1, false, |x| x.last_push = unix_now() - 25 * 3600);
    h.run(true);
    assert_eq!(h.mir.count(), 1);
    h.mir.reset();
    h.e.enqueue(Job::for_repo(
        Arc::new(upd(mk(1, "site"), "2026-05-01T00:00:00Z")),
        "sync",
        true,
        true,
    ));
    h.pump();
    assert_eq!(h.mir.count(), 1);
}

#[test]
fn refused_default_branch_is_tried_once() {
    let h = Harness::new(nothing);
    h.tgt
        .refuse_branch
        .store(true, std::sync::atomic::Ordering::SeqCst);
    h.src.set(Repo {
        default_branch: "master".into(),
        ..mk(1, "site")
    });
    h.run(false);
    h.src.set(Repo {
        default_branch: "master".into(),
        ..upd(mk(1, "site"), "2026-02-01T00:00:00Z")
    });
    h.run(false);
    let tries = h
        .tgt
        .calls()
        .iter()
        .filter(|c| c.contains("default_branch"))
        .count();
    assert_eq!(tries, 1);
    let x = h.entry(1).unwrap();
    assert_eq!(x.last_error, "");
    assert_eq!(x.refused_branch, "master");
}

#[test]
fn repo_deleted_mid_sync_is_not_a_failure() {
    let h = Harness::new(nothing);
    h.src.set(mk(1, "site"));
    *g(&h.mir.vanish) = Some(h.src.clone());
    h.run(false);
    assert_eq!(h.entry(1).map(|x| x.last_error).unwrap_or_default(), "");
    assert!(
        h.e.take_wake(),
        "the poller should be told to look at the deletion"
    );
}

#[test]
fn push_is_skipped_only_when_refs_are_known_and_nothing_forces_it() {
    let h = synced(nothing);
    g(&h.mir.lasts).clear();
    h.src.set(upd(mk(1, "site"), "2026-04-01T00:00:00Z"));
    h.run(false);
    assert_eq!(h.mir.last(), "refs-2026-01-01T00:00:00Z");
    h.src.set(upd(mk(1, "site"), "2026-05-01T00:00:00Z"));
    h.run(true);
    assert_eq!(h.mir.last(), "");
    h.e.enqueue(Job::for_repo(
        Arc::new(upd(mk(1, "site"), "2026-06-01T00:00:00Z")),
        "sync",
        false,
        true,
    ));
    h.pump();
    assert_eq!(h.mir.last(), "");
}
