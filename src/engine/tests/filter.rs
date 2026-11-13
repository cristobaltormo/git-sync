use super::{Harness, mk};
use crate::config::Config;
use crate::forge::Repo;

fn sel(f: impl Fn(&mut Config), r: Repo) -> bool {
    let h = Harness::new(f);
    h.e.selected(&r).0
}

#[test]
fn filters() {
    let site = || mk(1, "site");
    let mirror = || Repo {
        mirror: true,
        ..site()
    };
    assert!(sel(|_| {}, site()), "default");
    assert!(!sel(|_| {}, mirror()), "pull mirrors skipped");
    assert!(
        sel(|c| c.filter.skip_mirrors = false, mirror()),
        "mirrors allowed"
    );
    assert!(
        !sel(
            |c| c.filter.skip_forks = true,
            Repo {
                fork: true,
                ..site()
            }
        ),
        "forks"
    );
    assert!(
        !sel(
            |c| c.filter.skip_archived = true,
            Repo {
                archived: true,
                ..site()
            }
        ),
        "archived"
    );
    assert!(!sel(|c| c.filter.skip_private = true, site()), "private");
    assert!(
        !sel(|c| c.filter.exclude = vec!["si*".into()], site()),
        "exclude glob"
    );
    assert!(
        !sel(|c| c.filter.exclude = vec!["alice/site".into()], site()),
        "exclude full"
    );
    assert!(
        sel(|c| c.filter.include = vec!["alice/*".into()], site()),
        "include"
    );
    assert!(
        !sel(|c| c.filter.include = vec!["bob/*".into()], site()),
        "include miss"
    );
    assert!(
        !sel(|c| c.filter.topic = "mirror".into(), site()),
        "topic missing"
    );
    assert!(
        sel(
            |c| c.filter.topic = "mirror".into(),
            Repo {
                topics: vec!["mirror".into()],
                ..site()
            }
        ),
        "topic"
    );
    assert!(
        !sel(|c| c.filter.exclude = vec!["SITE".into()], site()),
        "patterns ignore case"
    );
}

#[test]
fn filter_topic_is_not_sent_to_github() {
    let h = Harness::new(|c| c.filter.topic = "gitsync".into());
    h.src.set(Repo {
        topics: vec!["gitsync".into(), "go".into()],
        ..mk(1, "site")
    });
    h.run(false);
    assert_eq!(h.tgt.repo("alice-gh/site").topics, vec!["go".to_string()]);
}

#[test]
fn excluded_is_left_alone() {
    let mut h = Harness::new(|_| {});
    h.src.set(mk(1, "site"));
    h.run(false);
    h.rebuild(|c| c.filter.exclude = vec!["site".into()]);
    h.run(false);
    assert!(h.entry(1).unwrap().excluded, "should be marked excluded");
    h.src.remove(1);
    super::g(&h.tgt.calls).clear();
    h.run(false);
    assert!(
        h.tgt.has("alice-gh/site"),
        "excluded repo must not be deleted"
    );
}
