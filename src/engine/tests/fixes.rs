use std::panic::panic_any;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::Duration;

use super::super::poll::fingerprint;
use super::super::settings::branch_refused;
use super::super::{Job, full_name, lock, unix_now};
use super::{Harness, g, mk, nothing, synced, upd, with_rules};
use crate::config::RepoRule;
use crate::forge::{Change, Event, ONEDEV_ROOT, Repo};
use crate::github;
use crate::mirror::{Options, Outcome};

fn ignore_rule() -> RepoRule {
    RepoRule {
        on_delete: "ignore".into(),
        ..RepoRule::default()
    }
}

fn unsynced_rule() -> RepoRule {
    RepoRule {
        sync: Some(false),
        ..RepoRule::default()
    }
}

#[test]
fn full_name_of_onedev_root() {
    assert_eq!(full_name(ONEDEV_ROOT, "proj"), "proj");
    assert_eq!(full_name("alice", "site"), "alice/site");
}

#[test]
fn onedev_root_rule_is_found_before_deleting() {
    let h = Harness::new(with_rules(&[("proj", ignore_rule())]));
    h.src.set(mk(2, "keep"));
    h.run(false);
    g(&h.tgt.repos).insert(
        "alice-gh/proj".into(),
        github::Repo {
            name: "proj".into(),
            owner: github::Owner {
                login: "alice-gh".into(),
            },
            ..github::Repo::default()
        },
    );
    h.e.state.update(7, true, |x| {
        x.owner = ONEDEV_ROOT.into();
        x.name = "proj".into();
        x.gh_owner = "alice-gh".into();
        x.gh_name = "proj".into();
        x.managed = true;
    });
    h.e.reconcile(false, "scan");
    assert!(h.tgt.has("alice-gh/proj"), "on_delete = ignore must win");
    assert!(h.entry(7).is_none());
    assert!(!h.tgt.calls().iter().any(|c| c.starts_with("delete")));
}

#[test]
fn sync_false_protects_a_vanished_repo() {
    let mut h = synced(nothing);
    h.src.set(mk(2, "keep"));
    h.run(false);
    h.src.remove(1);
    h.rebuild(with_rules(&[("alice/site", unsynced_rule())]));
    h.e.reconcile(false, "scan");
    assert!(
        h.tgt.has("alice-gh/site"),
        "sync = false must win over deletion"
    );
    let x = h.entry(1).unwrap();
    assert!(x.excluded && x.missing_since == 0);
    h.e.reconcile(false, "scan");
    assert!(h.tgt.has("alice-gh/site"));
}

#[test]
fn exclude_filter_protects_a_vanished_repo() {
    let mut h = synced(nothing);
    h.src.set(mk(2, "keep"));
    h.run(false);
    h.src.remove(1);
    h.rebuild(|c| c.filter.exclude = vec!["alice/si*".into()]);
    h.e.reconcile(false, "scan");
    assert!(h.tgt.has("alice-gh/site"));
    assert!(h.entry(1).unwrap().excluded);
}

#[test]
fn renamed_on_github_is_not_deleted() {
    let h = synced(nothing);
    g(&h.tgt.repos).get_mut("alice-gh/site").unwrap().name = "site-old".into();
    h.src.remove(1);
    h.src.set(mk(2, "keep"));
    h.e.reconcile(false, "scan");
    assert!(h.tgt.has("alice-gh/site"));
    assert!(!h.tgt.calls().iter().any(|c| c.starts_with("delete")));
    assert!(
        h.entry(1).is_none(),
        "the entry is dropped so it is logged once"
    );
}

#[test]
fn renamed_on_github_is_not_archived() {
    let h = synced(|c| c.sync.on_delete = "archive".into());
    g(&h.tgt.repos).get_mut("alice-gh/site").unwrap().name = "site-old".into();
    h.src.remove(1);
    h.src.set(mk(2, "keep"));
    h.e.reconcile(false, "scan");
    assert!(!h.tgt.repo("alice-gh/site").archived);
}

#[test]
fn one_account_going_empty_is_not_a_mass_delete() {
    let h = synced(nothing);
    h.src.set(mk(2, "other"));
    let mut org = mk(3, "tool");
    org.owner = "my-org".into();
    h.src.set(org);
    h.run(false);
    h.src.remove(1);
    h.src.remove(2);
    h.e.reconcile(false, "scan");
    assert!(h.tgt.has("alice-gh/site") && h.tgt.has("alice-gh/other"));
    assert_eq!(h.entry(1).unwrap().missing_since, 0);
}

#[test]
fn private_repo_checks_github_before_pushing() {
    let h = synced(nothing);
    g(&h.tgt.repos).get_mut("alice-gh/site").unwrap().private = false;
    h.src.set(upd(mk(1, "site"), "2026-02-01T00:00:00Z"));
    g(&h.events).clear();
    h.run(false);
    let ev = h.events();
    let close = ev.iter().position(|e| e == "patch private=true");
    let push = ev.iter().position(|e| e == "mirror");
    assert!(close.is_some() && close < push, "{ev:?}");
    assert!(h.tgt.repo("alice-gh/site").private);
}

#[test]
fn public_repo_keeps_the_cached_snapshot() {
    let h = Harness::new(nothing);
    let mut r = mk(1, "site");
    r.private = false;
    h.src.set(r.clone());
    h.run(false);
    let before = h.tgt.gets.load(Ordering::SeqCst);
    h.src.set(upd(r, "2026-02-01T00:00:00Z"));
    h.run(false);
    assert_eq!(h.tgt.gets.load(Ordering::SeqCst), before);
}

struct Bomb;

impl Drop for Bomb {
    fn drop(&mut self) {
        panic!("payload exploded");
    }
}

struct PanicMirror;

impl crate::engine::Mirror for PanicMirror {
    fn mirror(
        &self,
        r: &Repo,
        _: &str,
        _: &str,
        _: &str,
        _: Options,
    ) -> Result<Outcome, crate::Error> {
        if r.name == "bad" {
            panic_any(Bomb);
        }
        Ok(Outcome {
            summary: "up to date".into(),
            refs: String::new(),
        })
    }

    fn forget(&self, _: i64) {}
}

#[test]
fn worker_survives_a_panic_outside_the_job() {
    let h = Harness::new(nothing);
    let cfg = h.e.config();
    let e = crate::engine::Engine::new(cfg, h.src.clone(), h.tgt.clone(), Arc::new(PanicMirror))
        .unwrap();
    e.start();
    let bad = Arc::new(mk(1, "bad"));
    let good = Arc::new(mk(2, "good"));
    h.src.set((*bad).clone());
    h.src.set((*good).clone());
    e.enqueue(Job::for_repo(bad, "test", false, false));
    assert!(e.drain(Duration::from_secs(5)), "the key must be released");
    e.enqueue(Job::for_repo(good, "test", false, false));
    assert!(
        e.drain(Duration::from_secs(5)),
        "a worker must still be running"
    );
    assert!(e.state.get(2).is_some_and(|x| x.last_ok > 0));
    e.shutdown();
    assert_eq!(lock(&e.queue.lanes).running.len(), 0);
}

#[test]
fn idless_webhook_uses_the_known_id_and_respects_rules() {
    let h = Harness::new(with_rules(&[("alice/off", unsynced_rule())]));
    h.src.set(mk(1, "site"));
    h.run(false);
    let ev = |name: &str| Event {
        kind: Change::Changed,
        owner: "alice".into(),
        name: name.into(),
        id: 0,
    };
    h.e.handle_event(&ev("off"));
    assert_eq!(h.e.queued(), 0, "an ignored repo is not queued");
    h.e.handle_event(&ev("site"));
    h.e.handle_event(&ev("new"));
    let l = lock(&h.e.queue.lanes);
    assert!(
        l.pending.contains_key("1"),
        "known repo shares the poller key"
    );
    assert!(l.pending.contains_key("alice/new"));
}

#[test]
fn webhook_merge_drops_the_old_listing() {
    let h = Harness::new(nothing);
    h.e.enqueue(Job::for_repo(
        Arc::new(mk(1, "site")),
        "change",
        false,
        false,
    ));
    h.e.enqueue(Job {
        key: "1".into(),
        owner: "alice".into(),
        name: "site".into(),
        why: "webhook",
        ..Job::default()
    });
    let l = lock(&h.e.queue.lanes);
    let j = &l.pending["1"];
    assert!(j.repo.is_none() && j.owner == "alice" && j.name == "site");
}

#[test]
fn only_a_real_branch_refusal_is_remembered() {
    assert!(branch_refused(
        r#"{"message":"Validation Failed","errors":[{"field":"default_branch","code":"invalid"}]}"#
    ));
    assert!(branch_refused(
        "Cannot update default branch for an empty repository"
    ));
    assert!(!branch_refused(
        r#"{"message":"Visibility change in progress"}"#
    ));
    assert!(!branch_refused(
        r#"{"message":"Validation Failed","errors":[{"field":"visibility"}]}"#
    ));
}

#[test]
fn stuck_missing_repo_backs_off() {
    let h = synced(nothing);
    h.src.set(mk(2, "keep"));
    h.run(false);
    h.src.remove(1);
    h.src.owner_ok.store(false, Ordering::SeqCst);
    h.e.reconcile(false, "scan");
    h.e.reconcile(false, "scan");
    h.e.reconcile(false, "scan");
    assert_eq!(h.src.owner_calls.load(Ordering::SeqCst), 1);
    assert!(h.e.missing_wait().unwrap() >= Duration::from_secs(5));
    assert!(h.tgt.has("alice-gh/site"));
}

#[test]
fn grace_period_still_polls_quickly() {
    let h = synced(|c| c.sync.delete_grace = 60);
    h.src.set(mk(2, "keep"));
    h.run(false);
    h.src.remove(1);
    h.e.reconcile(false, "scan");
    assert_eq!(h.e.missing_wait(), Some(Duration::from_secs(5)));
    assert!(h.entry(1).unwrap().missing_since >= unix_now() - 5);
}

#[cfg(unix)]
#[test]
fn state_file_is_private_even_with_a_stale_temp() {
    use std::os::unix::fs::PermissionsExt;
    let h = Harness::new(nothing);
    let tmp = h.dir.path().join("state.json.tmp");
    std::fs::write(&tmp, "junk").unwrap();
    std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o644)).unwrap();
    h.src.set(mk(1, "site"));
    h.run(false);
    let mode = std::fs::metadata(h.dir.path().join("state.json"))
        .unwrap()
        .permissions()
        .mode();
    assert_eq!(mode & 0o777, 0o600);
    assert!(!tmp.exists());
}

#[test]
fn topic_order_does_not_change_the_fingerprint() {
    let mut a = mk(1, "site");
    a.topics = vec!["x".into(), "y".into()];
    let mut b = a.clone();
    b.topics = vec!["y".into(), "x".into()];
    let mut c = a.clone();
    c.topics = vec!["x".into(), "z".into()];
    let fp = |r: &Repo| fingerprint(&[Arc::new(r.clone())]);
    assert_eq!(fp(&a), fp(&b));
    assert_ne!(fp(&a), fp(&c));
}

#[test]
fn notify_url_is_redacted() {
    use super::super::notify::redact;
    assert_eq!(
        redact("https://hooks.slack.com/services/T0/B0/secret"),
        "https://hooks.slack.com/***"
    );
    assert_eq!(
        redact("https://user:pw@ntfy.sh/topic"),
        "https://ntfy.sh/***"
    );
    assert_eq!(redact("http://127.0.0.1:9"), "http://127.0.0.1:9");
}

#[test]
fn failed_notification_hides_the_path() {
    let srv = crate::testutil::TestServer::start(|_| crate::testutil::Reply::status(404));
    let url = format!("{}/hook/very-secret-token", srv.url());
    let err = crate::engine::send_test(&url, "json")
        .unwrap_err()
        .to_string();
    assert!(!err.contains("very-secret-token"), "{err}");
    assert!(err.ends_with("answered 404 Not Found"), "{err}");
}

#[test]
fn delete_limit_is_reported_once_per_window() {
    let h = Harness::new(nothing);
    assert!(!h.e.limit_reported());
    assert!(h.e.limit_reported());
}
