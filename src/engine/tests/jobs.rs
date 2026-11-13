use super::super::jobs::is_busy;
use super::super::unix_now;
use super::{Harness, g, mk, nothing};
use crate::error::Error;

#[test]
fn failure_backs_off_then_recovers() {
    let h = Harness::new(nothing);
    h.src.set(mk(1, "site"));
    *g(&h.mir.fail) = Some("network down".into());
    h.run(false);
    let x = h.entry(1).unwrap();
    assert!(x.last_error.contains("network down"), "error not recorded");
    assert!(x.retry_at > unix_now(), "no backoff");
    assert_eq!(x.attempts, 1);
    h.e.reconcile(false, "scan");
    assert_eq!(h.e.queued(), 0);
    *g(&h.mir.fail) = None;
    h.e.state.update(1, false, |x| x.retry_at = unix_now() - 1);
    h.run(false);
    let x = h.entry(1).unwrap();
    assert_eq!(x.last_error, "");
    assert_eq!(x.attempts, 0);
}

#[test]
fn backoff_grows_and_caps() {
    let h = Harness::new(nothing);
    h.src.set(mk(1, "site"));
    *g(&h.mir.fail) = Some("network down".into());
    let mut waits = Vec::new();
    for _ in 0..5 {
        h.e.state.update(1, false, |x| x.retry_at = 0);
        h.run(false);
        waits.push(h.entry(1).unwrap().retry_at - unix_now());
    }
    let want = [30, 120, 600, 1800, 1800];
    for (got, want) in waits.iter().zip(want) {
        assert!((want - 2..=want).contains(got), "{waits:?}");
    }
}

#[test]
fn busy_github_retries_quickly() {
    let h = Harness::new(nothing);
    h.src.set(mk(1, "site"));
    *g(&h.mir.fail) =
        Some("git push failed: remote: Repository 'x/site' is disabled. | fatal: 403".into());
    h.run(false);
    let x = h.entry(1).unwrap();
    let wait = x.retry_at - unix_now();
    assert!(
        wait > 0 && wait <= 12,
        "a busy repo should be retried in about 10s, got {wait}s"
    );
    assert_eq!(
        h.e.next_retry(0),
        Some(x.retry_at),
        "the poller must know when to come back"
    );
    assert!(is_busy(&Error::api(
        422,
        "Failed to update visibility. A previous visibility change is still in progress."
    )));
    assert!(
        !is_busy(&Error::api(422, "Validation failed")),
        "other 422s are real errors"
    );
}

#[test]
fn blocked_repos_wait_ten_minutes() {
    let h = Harness::new(nothing);
    g(&h.tgt.repos).insert("alice-gh/site".into(), Default::default());
    h.src.set(mk(1, "site"));
    h.run(false);
    let x = h.entry(1).unwrap();
    let wait = x.retry_at - unix_now();
    assert!((598..=600).contains(&wait), "{wait}");
}
