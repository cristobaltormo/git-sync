use std::sync::atomic::Ordering;

use super::super::unix_now;
use super::{Harness, g, mk, nothing, synced};

#[test]
fn delete_after_grace() {
    let h = synced(|c| c.sync.delete_grace = 30);
    h.src.remove(1);
    h.e.reconcile(false, "scan");
    assert!(h.tgt.has("alice-gh/site"), "grace not over yet");
    h.e.state
        .update(1, false, |x| x.missing_since = unix_now() - 60);
    h.e.reconcile(false, "scan");
    assert!(!h.tgt.has("alice-gh/site"), "should be deleted");
    assert!(h.entry(1).is_none(), "entry should be gone");
    assert!(h.mir.calls().contains(&"forget 1".to_string()));
}

#[test]
fn confirm_before_delete() {
    let h = synced(nothing);
    *g(&h.src.hide) = Some(1);
    h.e.reconcile(false, "scan");
    assert!(
        h.tgt.has("alice-gh/site"),
        "must not delete when the repo still exists"
    );
    assert_eq!(h.entry(1).unwrap().missing_since, 0);
}

#[test]
fn empty_listing_is_not_a_mass_delete() {
    let h = synced(nothing);
    h.src.set(mk(2, "other"));
    h.run(false);
    h.src.clear();
    h.e.reconcile(false, "scan");
    assert!(
        h.tgt.has("alice-gh/site") && h.tgt.has("alice-gh/other"),
        "nothing may be deleted"
    );
}

#[test]
fn single_repo_can_be_deleted() {
    let h = synced(nothing);
    h.src.clear();
    h.e.reconcile(false, "scan");
    assert!(
        !h.tgt.has("alice-gh/site"),
        "the only repo should be deletable"
    );
}

#[test]
fn owner_lookup_failure_blocks_delete() {
    let h = synced(nothing);
    h.src.set(mk(2, "other"));
    h.run(false);
    h.src.remove(1);
    h.src.owner_ok.store(false, Ordering::SeqCst);
    h.e.reconcile(false, "scan");
    assert!(h.tgt.has("alice-gh/site"), "owner lookup failed: keep it");
}

#[test]
fn archive_mode() {
    let h = synced(|c| c.sync.on_delete = "archive".into());
    h.src.remove(1);
    h.src.set(mk(2, "keep"));
    h.e.reconcile(false, "scan");
    assert!(
        h.tgt.repo("alice-gh/site").archived,
        "should be archived, not deleted"
    );
}

#[test]
fn ignore_mode() {
    let h = synced(|c| c.sync.on_delete = "ignore".into());
    h.src.remove(1);
    h.src.set(mk(2, "keep"));
    h.e.reconcile(false, "scan");
    assert!(!h.tgt.repo("alice-gh/site").archived, "must be left alone");
    assert!(h.entry(1).is_none(), "entry should be forgotten");
}

#[test]
fn delete_limit() {
    let h = Harness::new(|c| c.sync.delete_limit = 2);
    for i in 1..=4 {
        h.src.set(mk(i, &format!("r{i}")));
    }
    h.run(false);
    h.src.clear();
    h.src.set(mk(9, "survivor"));
    h.e.reconcile(false, "scan");
    let gone = ["r1", "r2", "r3", "r4"]
        .iter()
        .filter(|k| !h.tgt.has(&format!("alice-gh/{k}")))
        .count();
    assert_eq!(gone, 2);
}

#[test]
fn never_deletes_foreign_repo() {
    let h = synced(nothing);
    g(&h.tgt.repos)
        .get_mut("alice-gh/site")
        .unwrap()
        .owner
        .login = "someone-else".into();
    h.src.remove(1);
    h.src.set(mk(2, "keep"));
    h.e.reconcile(false, "scan");
    assert!(
        h.tgt.has("alice-gh/site"),
        "must not delete what we do not own"
    );
}

#[test]
fn listing_failure_skips_scan() {
    let h = synced(nothing);
    *g(&h.src.list_err) = Some("boom".into());
    assert!(
        !h.e.reconcile(false, "scan"),
        "reconcile should report failure"
    );
    assert!(
        h.tgt.has("alice-gh/site"),
        "nothing may be deleted on a failed listing"
    );
}
