use super::{nothing, synced};

#[test]
fn state_survives_restart() {
    let mut h = synced(nothing);
    h.rebuild(nothing);
    let x = h.entry(1).expect("state not reloaded");
    assert!(x.managed && x.gh.is_some(), "state not reloaded");
    h.e.reconcile(false, "scan");
    assert_eq!(h.e.queued(), 0);
}
