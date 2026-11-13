use std::time::{Duration, Instant};

use super::super::pulls::pull_message;
use super::{Harness, g, mk, synced, test_config};
use crate::github::Pull;
use crate::testutil::{Reply, TestServer};

fn pull(owner: &str, repo: &str, number: i64) -> Pull {
    Pull {
        owner: owner.into(),
        repo: repo.into(),
        number,
        url: format!("https://github.com/{owner}/{repo}/pull/{number}"),
        ..Pull::default()
    }
}

#[test]
fn pull_requests_are_never_touched_by_default() {
    let h = synced(|_| {});
    g(&h.tgt.pulls).insert("alice-gh".into(), vec![pull("alice-gh", "site", 1)]);
    h.e.check_pulls();
    assert!(g(&h.tgt.comments).is_empty());
}

#[test]
fn pull_request_comment_only_for_new_ones() {
    let h = synced(|c| c.pulls.mode = "comment".into());
    g(&h.tgt.pulls).insert("alice-gh".into(), vec![pull("alice-gh", "site", 1)]);
    h.e.check_pulls();
    assert!(g(&h.tgt.comments).is_empty());

    g(&h.tgt.pulls)
        .get_mut("alice-gh")
        .unwrap()
        .extend([pull("alice-gh", "site", 2), pull("alice-gh", "not-ours", 3)]);
    h.e.check_pulls();
    h.e.check_pulls();
    let comments = g(&h.tgt.comments).clone();
    assert_eq!(comments.len(), 1);
    assert!(
        comments[0].starts_with("alice-gh/site#2: This repository is a read-only mirror"),
        "{}",
        comments[0]
    );
    let known = h.e.state.with(|st| st.pulls.clone());
    assert_eq!(known.len(), 2);
    g(&h.tgt.pulls).clear();
    h.e.check_pulls();
    assert!(
        h.e.state.with(|st| st.pulls.is_empty()),
        "closed pulls are forgotten"
    );
}

#[test]
fn custom_pull_message_replaces_placeholders_once() {
    let dir = crate::testutil::TempDir::new();
    let cfg = test_config(dir.path().to_str().unwrap(), &|c| {
        c.pulls.message = "see {url} for {repo} ({repo})".into()
    });
    assert_eq!(
        pull_message(&cfg, "alice/site"),
        "see https://git.example.com/alice/site for alice/site (alice/site)"
    );
    assert_eq!(
        pull_message(&cfg, "{url}"),
        "see https://git.example.com/{url} for {url} ({url})"
    );
}

#[test]
fn notifications_reach_the_webhook() {
    let srv = TestServer::start(|_| Reply::status(200));
    let url = srv.url();
    let h = Harness::new(move |c| {
        c.filter.new_repos = "review".into();
        c.notify.url = url.clone();
        c.notify.format = "slack".into();
    });
    h.src.set(mk(1, "site"));
    h.run(false);
    h.run(false);
    let deadline = Instant::now() + Duration::from_secs(2);
    while Instant::now() < deadline && srv.requests().is_empty() {
        std::thread::sleep(Duration::from_millis(10));
    }
    std::thread::sleep(Duration::from_millis(50));
    let reqs = srv.requests();
    assert_eq!(reqs.len(), 1);
    let body = String::from_utf8_lossy(&reqs[0].body).into_owned();
    assert!(
        body.contains("alice/site") && body.contains(r#""text""#),
        "{body}"
    );
}
