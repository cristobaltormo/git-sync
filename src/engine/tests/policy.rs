use super::{Harness, mk, upd, with_rules};
use crate::config::RepoRule;
use crate::forge::{ROLE_ADMIN, ROLE_WRITE, Repo};

#[test]
fn new_repos_review_keeps_them_out_until_decided() {
    let mut h = Harness::new(|c| c.filter.new_repos = "review".into());
    h.src.set(mk(1, "site"));
    h.run(false);
    assert!(
        !h.tgt.has("alice-gh/site"),
        "undecided repo must not reach GitHub"
    );
    let x = h.entry(1).expect("the repo must be recorded as pending");
    assert!(x.pending != 0 && !x.managed);
    assert!(h.tgt.calls().is_empty());

    let rules = with_rules(&[(
        "alice/site",
        RepoRule {
            sync: Some(true),
            ..RepoRule::default()
        },
    )]);
    h.rebuild(|c| {
        c.filter.new_repos = "review".into();
        rules(c);
    });
    h.run(false);
    assert!(h.tgt.has("alice-gh/site"), "allowed repo must be synced");
    assert_eq!(h.entry(1).unwrap().pending, 0);
}

#[test]
fn new_repos_ignore_is_silent() {
    let h = Harness::new(|c| c.filter.new_repos = "ignore".into());
    h.src.set(mk(1, "site"));
    h.run(false);
    assert!(h.entry(1).is_none(), "ignored repos leave no trace");
    assert!(!h.tgt.has("alice-gh/site"), "not synced");
}

#[test]
fn existing_repos_are_grandfathered() {
    let mut h = Harness::new(|_| {});
    h.src.set(mk(1, "site"));
    h.run(false);
    h.src.set(mk(2, "fresh"));
    h.rebuild(|c| c.filter.new_repos = "review".into());
    h.src.set(upd(mk(1, "site"), "2027-01-01T00:00:00Z"));
    h.run(false);
    assert!(
        h.tgt.has("alice-gh/site"),
        "already synced repo keeps syncing"
    );
    assert!(
        !h.tgt.has("alice-gh/fresh"),
        "repo that appeared later waits"
    );
    assert!(h.e.decide(&mk(2, "fresh")).pending);
}

#[test]
fn explicit_decision_beats_filters() {
    let r = Repo {
        fork: true,
        ..mk(1, "site")
    };
    let rules = with_rules(&[(
        "alice/site",
        RepoRule {
            sync: Some(true),
            ..RepoRule::default()
        },
    )]);
    let h = Harness::new(|c| {
        c.filter.skip_forks = true;
        rules(c);
    });
    assert!(h.e.decide(&r).sync, "explicit allow wins over skip_forks");
    let h = Harness::new(with_rules(&[(
        "alice/site",
        RepoRule {
            sync: Some(false),
            ..RepoRule::default()
        },
    )]));
    let v = h.e.decide(&mk(1, "site"));
    assert!(
        !v.sync && !v.pending && v.why.contains("ignored"),
        "explicit ignore"
    );
}

#[test]
fn scope_admin() {
    let h = Harness::new(|c| c.filter.scope = "admin".into());
    let a = Repo {
        role: ROLE_ADMIN,
        ..mk(1, "a")
    };
    let b = Repo {
        role: ROLE_WRITE,
        ..mk(2, "b")
    };
    assert!(h.e.decide(&a).sync, "admin");
    assert!(!h.e.decide(&b).sync, "collaborator");
    assert!(
        h.e.decide(&mk(3, "c")).sync,
        "unknown role is not held against the repo"
    );
}

#[test]
fn per_repo_overrides() {
    let h = Harness::new(with_rules(&[(
        "alice/site",
        RepoRule {
            name: "site-mirror".into(),
            keep_private: Some(true),
            metadata: Some(false),
            ..RepoRule::default()
        },
    )]));
    h.src.set(Repo {
        private: false,
        description: "hello".into(),
        topics: vec!["go".into()],
        ..mk(1, "site")
    });
    h.run(false);
    let g = h.tgt.repo("alice-gh/site-mirror");
    assert!(g.private, "keep_private holds the copy private");
    assert_eq!(g.description.as_deref(), Some(""));
    assert!(g.topics.is_empty());
}

#[test]
fn per_repo_on_delete() {
    let h = Harness::new(with_rules(&[(
        "alice/site",
        RepoRule {
            on_delete: "ignore".into(),
            ..RepoRule::default()
        },
    )]));
    h.src.set(mk(1, "site"));
    h.run(false);
    h.src.remove(1);
    h.run(false);
    assert!(
        h.tgt.has("alice-gh/site"),
        "on_delete = ignore keeps the copy"
    );
}

#[test]
fn prune_option_reaches_the_mirror() {
    let h = Harness::new(with_rules(&[(
        "alice/site",
        RepoRule {
            prune: Some(false),
            ..RepoRule::default()
        },
    )]));
    h.src.set(mk(1, "site"));
    h.src.set(mk(2, "other"));
    h.run(false);
    let calls = h.mir.calls();
    let opts = super::g(&h.mir.opts).clone();
    for (c, o) in calls.iter().zip(&opts) {
        if c.contains("alice/site") {
            assert!(!o.prune, "prune = false must be passed to the mirror");
        }
        if c.contains("alice/other") {
            assert!(o.prune, "prune stays on for repos without the option");
        }
    }
    assert_eq!(calls.len(), 2);
}
