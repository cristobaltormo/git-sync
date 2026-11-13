use super::{Harness, mk, nothing, upd};
use crate::github;

#[test]
fn rename() {
    let h = Harness::new(nothing);
    h.src.set(mk(1, "old"));
    h.run(false);
    h.src.set(upd(mk(1, "new"), "2026-02-01T00:00:00Z"));
    h.run(false);
    assert!(
        h.tgt.has("alice-gh/new") && !h.tgt.has("alice-gh/old"),
        "rename not applied"
    );
    assert_eq!(h.entry(1).unwrap().gh_name, "new");
    for c in &h.tgt.calls()[1..] {
        assert!(!c.starts_with("create"), "renamed repo was created again");
    }
}

#[test]
fn rename_onto_existing_is_blocked() {
    let h = Harness::new(nothing);
    h.src.set(mk(1, "old"));
    h.run(false);
    super::g(&h.tgt.repos).insert(
        "alice-gh/new".into(),
        github::Repo {
            name: "new".into(),
            ..github::Repo::default()
        },
    );
    h.src.set(upd(mk(1, "new"), "2026-02-01T00:00:00Z"));
    h.run(false);
    assert!(
        h.tgt.has("alice-gh/old") && h.tgt.has("alice-gh/new"),
        "nothing may be touched"
    );
    assert!(
        h.entry(1).unwrap().last_error.contains("already exists"),
        "expected a clear error"
    );
}
