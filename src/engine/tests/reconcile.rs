use std::sync::Arc;

use super::super::poll::fingerprint;
use super::mk;
use crate::forge::Repo;

type Mutate = Box<dyn Fn(&mut Repo)>;

#[test]
fn fingerprint_sees_every_mirrored_field() {
    let base = || vec![Arc::new(mk(1, "a")), Arc::new(mk(2, "b"))];
    let f0 = fingerprint(&base());
    assert_eq!(
        f0,
        fingerprint(&[Arc::new(mk(2, "b")), Arc::new(mk(1, "a"))])
    );
    let changes: Vec<(&str, Mutate)> = vec![
        ("visibility", Box::new(|r| r.private = false)),
        ("description", Box::new(|r| r.description = "x".into())),
        (
            "website",
            Box::new(|r| r.website = "https://x.example".into()),
        ),
        ("archived", Box::new(|r| r.archived = true)),
        ("branch", Box::new(|r| r.default_branch = "trunk".into())),
        ("topics", Box::new(|r| r.topics = vec!["go".into()])),
        ("rename", Box::new(|r| r.name = "renamed".into())),
        (
            "push",
            Box::new(|r| r.updated = "2031-01-01T00:00:00Z".into()),
        ),
        ("empty", Box::new(|r| r.empty = true)),
        ("role", Box::new(|r| r.role = crate::forge::ROLE_READ)),
    ];
    for (name, mutate) in changes {
        let mut repos = base();
        mutate(Arc::make_mut(&mut repos[0]));
        assert_ne!(
            fingerprint(&repos),
            f0,
            "{name} must change the fingerprint"
        );
    }
    assert_ne!(
        fingerprint(&base()[..1]),
        f0,
        "a deleted repo must change the fingerprint"
    );
}
