use std::collections::BTreeMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant, UNIX_EPOCH};

use super::super::notify::rfc3339_utc;
use super::super::{Entry, Job, State, read_state, send_test, unix_now};
use super::{Harness, g, mk, nothing, synced, test_config, with_rules};
use crate::config::RepoRule;
use crate::forge::{Change, Event};
use crate::github::Snap;
use crate::testutil::{Reply, TestServer};

fn sample() -> State {
    let mut st = State {
        system_hook: 42,
        pulls_ready: true,
        ..State::default()
    };
    st.pulls.insert("o/r#1".into(), 5);
    st.repos.insert(
        1,
        Entry {
            owner: "alice".into(),
            name: "site".into(),
            gh_owner: "alice-gh".into(),
            gh_name: "site".into(),
            managed: true,
            sig: "abc".into(),
            updated: "2026-01-01T00:00:00Z".into(),
            gh: Some(Snap {
                private: true,
                default_branch: "main".into(),
                ..Snap::default()
            }),
            last_ok: 100,
            last_error: "a<b & c> \u{2028}".into(),
            attempts: 2,
            ..Entry::default()
        },
    );
    st.repos.insert(
        10,
        Entry {
            owner: "x".into(),
            name: "y".into(),
            ..Entry::default()
        },
    );
    st.repos.insert(
        2,
        Entry {
            owner: "p".into(),
            name: "q".into(),
            pending: 7,
            excluded: true,
            ..Entry::default()
        },
    );
    st
}

const GO_JSON: &str = r#"{
 "version": 1,
 "system_hook": 42,
 "pulls": {
  "o/r#1": 5
 },
 "pulls_ready": true,
 "repos": {
  "1": {
   "owner": "alice",
   "name": "site",
   "gh_owner": "alice-gh",
   "gh_name": "site",
   "managed": true,
   "sig": "abc",
   "updated": "2026-01-01T00:00:00Z",
   "gh": {
    "private": true,
    "archived": false,
    "description": "",
    "homepage": "",
    "default_branch": "main",
    "topics": null
   },
   "last_ok": 100,
   "last_error": "a@003cb @0026 c@003e @2028",
   "attempts": 2
  },
  "10": {
   "owner": "x",
   "name": "y",
   "managed": false
  },
  "2": {
   "owner": "p",
   "name": "q",
   "managed": false,
   "excluded": true,
   "pending": 7
  }
 }
}"#;

#[test]
fn state_json_is_byte_compatible_with_go() {
    let st = sample();
    let blob = st.encode().unwrap();
    let want = GO_JSON.replace('@', &format!("{}u", char::from(92)));
    assert_eq!(String::from_utf8(blob).unwrap(), want);
    let back: State = serde_json::from_str(&want).unwrap();
    assert_eq!(back, st);
    let empty = State::default().encode().unwrap();
    assert_eq!(
        String::from_utf8(empty).unwrap(),
        "{\n \"version\": 1,\n \"repos\": {}\n}"
    );
}

#[test]
fn state_written_by_go_loads_with_nulls_and_unknown_keys() {
    let st: State = serde_json::from_str(
        r#"{"version":1,"pulls":null,"repos":{"5":{"owner":"a","name":"b","managed":true,"gh":null,"future":1}},"extra":true}"#,
    )
    .unwrap();
    assert!(st.repos[&5].managed && st.repos[&5].gh.is_none());
    let st: State = serde_json::from_str(r#"{"version":1,"repos":null}"#).unwrap();
    assert!(st.repos.is_empty());
}

#[test]
fn state_file_round_trips_through_the_engine() {
    let mut h = synced(nothing);
    h.e.state.save();
    let dir = h.dir.path().to_str().unwrap().to_string();
    let st = read_state(&dir).unwrap();
    assert!(st.repos[&1].managed);
    let raw = std::fs::read(h.dir.path().join("state.json")).unwrap();
    assert_eq!(raw, st.encode().unwrap());
    assert!(!h.dir.path().join("state.json.tmp").exists());
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(h.dir.path().join("state.json"))
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o600);
    }
    h.rebuild(nothing);
    assert_eq!(h.e.entries().len(), 1);
}

#[test]
fn broken_state_is_a_config_error() {
    let dir = crate::testutil::TempDir::new();
    std::fs::write(dir.path().join("state.json"), "{nope").unwrap();
    let cfg = test_config(dir.path().to_str().unwrap(), &nothing);
    let h = Harness::new(nothing);
    let err = super::super::Engine::new(Arc::new(cfg), h.src.clone(), h.tgt.clone(), h.mir.clone())
        .err()
        .unwrap();
    assert!(err.is_config());
    assert!(err.to_string().starts_with("cannot read "), "{err}");
}

#[test]
fn concurrent_saves_never_leave_a_torn_file() {
    let h = synced(nothing);
    for i in 2..200 {
        h.e.state.update(i, true, |x| {
            x.owner = "alice".into();
            x.name = format!("repo-{i}");
        });
    }
    std::thread::scope(|s| {
        for _ in 0..4 {
            s.spawn(|| {
                for _ in 0..25 {
                    h.e.state.save();
                }
            });
        }
    });
    let st = read_state(h.dir.path().to_str().unwrap()).unwrap();
    assert_eq!(st.repos.len(), 199);
}

#[test]
fn dry_run_never_writes_state() {
    let h = Harness::new(|c| c.sync.dry_run = true);
    h.src.set(mk(1, "site"));
    h.run(false);
    assert!(!h.dir.path().join("state.json").exists());
}

#[test]
fn timestamps_are_rfc3339_utc() {
    assert_eq!(rfc3339_utc(UNIX_EPOCH), "1970-01-01T00:00:00Z");
    assert_eq!(
        rfc3339_utc(UNIX_EPOCH + Duration::from_secs(1_792_465_200)),
        "2026-10-20T03:00:00Z"
    );
    assert_eq!(
        rfc3339_utc(UNIX_EPOCH + Duration::from_secs(951_782_400)),
        "2000-02-29T00:00:00Z"
    );
}

#[test]
fn notification_formats() {
    let srv = TestServer::start(|r| {
        if r.target == "/missing" {
            Reply::status(404)
        } else {
            Reply::status(204)
        }
    });
    send_test(&srv.url(), "json").unwrap();
    send_test(&format!("{}/t", srv.url()), "text").unwrap();
    send_test(&format!("{}/d", srv.url()), "discord").unwrap();
    let reqs = srv.requests();
    let json: BTreeMap<String, String> = serde_json::from_slice(&reqs[0].body).unwrap();
    assert_eq!(
        json.keys().collect::<Vec<_>>(),
        ["event", "message", "repo", "time"]
    );
    assert_eq!(json["event"], "pending");
    assert_eq!(json["repo"], "my-org/example");
    assert!(json["time"].ends_with('Z'));
    assert_eq!(
        String::from_utf8_lossy(&reqs[1].body),
        "New repository my-org/example is waiting for your decision. Run `gitsync repos` to sync or ignore it."
    );
    assert!(String::from_utf8_lossy(&reqs[2].body).starts_with(r#"{"content":"New repository"#));
    let err = send_test(&format!("{}/missing", srv.url()), "json").unwrap_err();
    assert!(err.to_string().ends_with("answered 404 Not Found"), "{err}");
}

#[test]
fn failing_notification_after_three_attempts() {
    let srv = TestServer::start(|_| Reply::status(200));
    let url = srv.url();
    let h = Harness::new(move |c| c.notify.url = url.clone());
    h.src.set(mk(1, "site"));
    *g(&h.mir.fail) = Some("network down".into());
    for _ in 0..4 {
        h.e.state.update(1, false, |x| x.retry_at = 0);
        h.run(false);
    }
    let deadline = Instant::now() + Duration::from_secs(2);
    while Instant::now() < deadline && srv.requests().is_empty() {
        std::thread::sleep(Duration::from_millis(10));
    }
    std::thread::sleep(Duration::from_millis(50));
    let reqs = srv.requests();
    assert_eq!(reqs.len(), 1);
    let body = String::from_utf8_lossy(&reqs[0].body).into_owned();
    assert!(
        body.contains("keeps failing to sync: network down"),
        "{body}"
    );
}

#[test]
fn stale_retries_do_not_keep_the_poller_busy() {
    let h = Harness::new(nothing);
    let now = unix_now();
    h.e.state.update(1, true, |x| {
        x.owner = "alice".into();
        x.name = "site".into();
        x.last_error = "boom".into();
        x.retry_at = now - 5;
    });
    assert_eq!(h.e.next_retry(0), Some(now - 5));
    assert_eq!(
        h.e.next_retry(now),
        None,
        "already handled by the last scan"
    );
    h.e.state.update(1, false, |x| x.retry_at = now + 30);
    assert_eq!(h.e.next_retry(now), Some(now + 30));
}

#[test]
fn webhook_for_an_undecided_repo_does_not_mark_it_excluded() {
    let h = Harness::new(|c| c.filter.new_repos = "review".into());
    h.src.set(mk(1, "site"));
    h.run(false);
    h.e.handle_event(&Event {
        kind: Change::Changed,
        owner: "alice".into(),
        name: "site".into(),
        id: 1,
    });
    let job = super::super::lock(&h.e.queue.lanes)
        .pending
        .remove("1")
        .unwrap();
    super::super::lock(&h.e.queue.lanes).order.clear();
    h.e.execute(&job);
    let x = h.entry(1).unwrap();
    assert!(x.pending != 0 && !x.excluded);
}

#[test]
fn excluded_repo_is_marked_once() {
    let mut h = synced(nothing);
    let rules = with_rules(&[(
        "alice/site",
        RepoRule {
            sync: Some(false),
            ..RepoRule::default()
        },
    )]);
    h.rebuild(rules);
    h.run(false);
    assert!(h.entry(1).unwrap().excluded);
    h.run(true);
    assert_eq!(h.e.queued(), 0);
    assert!(h.tgt.has("alice-gh/site"));
}

#[test]
fn long_descriptions_are_cut_on_a_character_boundary() {
    let h = Harness::new(nothing);
    let desc = format!("{}é{}", "a".repeat(346), "b".repeat(20));
    h.src.set(crate::forge::Repo {
        description: desc,
        ..mk(1, "site")
    });
    h.run(false);
    let got = h.tgt.repo("alice-gh/site").description.unwrap();
    assert_eq!(got, format!("{}...", "a".repeat(346)));
}

#[test]
fn poll_loop_reacts_to_wake_and_stop_at_once() {
    let h = Harness::new(|c| {
        c.sync.poll_interval = 3600;
        c.sync.workers = 1;
    });
    h.src.set(mk(1, "site"));
    h.e.start();
    let ticks = Arc::new(AtomicUsize::new(0));
    let e = h.e.clone();
    let t = ticks.clone();
    let poller = std::thread::spawn(move || {
        e.poll_loop(|| {
            t.fetch_add(1, Ordering::SeqCst);
        })
    });
    let wait_for = |what: &dyn Fn() -> bool| {
        let end = Instant::now() + Duration::from_secs(5);
        while !what() {
            assert!(Instant::now() < end, "timed out");
            std::thread::sleep(Duration::from_millis(5));
        }
    };
    wait_for(&|| ticks.load(Ordering::SeqCst) >= 1);
    assert!(h.e.drain(Duration::from_secs(5)));
    assert_eq!(h.mir.count(), 1);
    assert_eq!(h.e.hook_mode(), "none");

    h.src.set(upd_site());
    h.e.wake();
    wait_for(&|| ticks.load(Ordering::SeqCst) >= 2);
    assert!(h.e.drain(Duration::from_secs(5)));
    assert_eq!(h.mir.count(), 2);

    let t0 = Instant::now();
    h.e.shutdown();
    poller.join().unwrap();
    assert!(t0.elapsed() < Duration::from_secs(2));
    assert_eq!(ticks.load(Ordering::SeqCst), 2);
}

fn upd_site() -> crate::forge::Repo {
    super::upd(mk(1, "site"), "2026-09-09T00:00:00Z")
}

#[test]
fn reload_clears_retries_and_wakes_the_poller() {
    let h = Harness::new(nothing);
    h.src.set(mk(1, "site"));
    *g(&h.mir.fail) = Some("network down".into());
    h.run(false);
    assert!(h.entry(1).unwrap().retry_at > 0);
    let mut next = test_config("/elsewhere", &|c| c.sync.workers = 3);
    next.paths.state_dir = "/elsewhere".into();
    h.e.reload(next);
    assert_eq!(h.e.config().sync.workers, 3);
    assert_eq!(
        h.e.config().paths.state_dir,
        h.dir.path().to_str().unwrap(),
        "the state dir never moves at runtime"
    );
    assert_eq!(h.entry(1).unwrap().retry_at, 0);
    assert!(h.e.take_wake());
}

#[test]
fn job_merging_keeps_force_and_fresh_data() {
    let h = Harness::new(nothing);
    h.e.enqueue(Job {
        key: "1".into(),
        owner: "alice".into(),
        name: "site".into(),
        why: "webhook",
        ..Job::default()
    });
    h.e.enqueue(Job::for_repo(Arc::new(mk(1, "site")), "sync", true, true));
    let l = super::super::lock(&h.e.queue.lanes);
    let j = &l.pending["1"];
    assert!(j.repo.is_some() && j.verify && j.force);
    assert_eq!(j.why, "webhook");
}
