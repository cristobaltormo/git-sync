use std::time::{Duration, Instant};

use serde_json::Value;

use super::filter::unwanted;
use super::state::Entry;
use super::{Engine, eq_fold, full_name, lock, unix_now};
use crate::github::Fields;

const DELETE_WINDOW: Duration = Duration::from_secs(3600);
const MISSING_RETRY: Duration = Duration::from_secs(5);
const MISSING_RETRY_MAX: Duration = Duration::from_secs(300);

#[derive(Debug, Clone, Copy)]
pub(super) struct Backoff {
    next: Instant,
    step: u32,
}

enum Outcome {
    Done,
    Later,
}

impl Engine {
    pub(super) fn handle_missing(&self, id: i64, x: Entry) {
        if !x.managed {
            self.forget_missing(id);
            self.state.drop_entry(id);
            self.state.save();
            return;
        }
        let full = full_name(&x.owner, &x.name);
        if let Some(why) = unwanted(&self.config(), &x.owner, &x.name) {
            self.leave_unsynced(id, &full, why);
            return;
        }
        let now = unix_now();
        let grace = self.config().sync.delete_grace;
        let mut first = x.missing_since;
        if first == 0 {
            first = now;
            self.state.update(id, false, |y| y.missing_since = now);
            if grace > 0 {
                infof!("{full} is missing at the source, waiting {grace}s before touching GitHub");
            }
        }
        if now - first < grace || self.missing_deferred(id) {
            return;
        }
        match self.src.get(&x.owner, &x.name) {
            Err(e) => {
                self.defer(
                    id,
                    &format!("{full}: cannot confirm the removal ({e}), leaving it"),
                );
                return;
            }
            Ok(Some(_)) => {
                self.forget_missing(id);
                self.state.update(id, false, |y| y.missing_since = 0);
                return;
            }
            Ok(None) => {}
        }
        if !matches!(self.src.owner_exists(&x.owner), Ok(true)) {
            self.defer(
                id,
                &format!("{full}: owner lookup failed, not treating it as deleted"),
            );
            return;
        }
        match self.apply_removal(id, &x) {
            Outcome::Done => self.forget_missing(id),
            Outcome::Later => {
                self.bump(id);
            }
        }
    }

    fn leave_unsynced(&self, id: i64, full: &str, why: &str) {
        self.forget_missing(id);
        self.state.update(id, false, |y| {
            y.excluded = true;
            y.missing_since = 0;
        });
        self.state.save();
        infof!("{full}: no longer synced ({why}); the GitHub copy is left alone");
    }

    fn missing_deferred(&self, id: i64) -> bool {
        lock(&self.missing)
            .get(&id)
            .is_some_and(|b| Instant::now() < b.next)
    }

    fn bump(&self, id: i64) -> bool {
        let mut m = lock(&self.missing);
        let b = m.entry(id).or_insert(Backoff {
            next: Instant::now(),
            step: 0,
        });
        let delay = MISSING_RETRY
            .saturating_mul(1 << b.step.min(6))
            .min(MISSING_RETRY_MAX);
        b.step += 1;
        b.next = Instant::now() + delay;
        b.step == 1
    }

    fn defer(&self, id: i64, msg: &str) {
        if self.bump(id) {
            warnf!("{msg}");
        } else {
            debugf!("{msg}");
        }
    }

    fn forget_missing(&self, id: i64) {
        lock(&self.missing).remove(&id);
    }

    pub(super) fn missing_wait(&self) -> Option<Duration> {
        let now = Instant::now();
        let m = lock(&self.missing);
        self.state.with(|st| {
            st.repos
                .iter()
                .filter(|(_, x)| x.missing_since != 0 && !x.excluded)
                .map(|(id, _)| {
                    m.get(id).map_or(MISSING_RETRY, |b| {
                        b.next.saturating_duration_since(now).max(MISSING_RETRY)
                    })
                })
                .min()
        })
    }

    fn within_delete_limit(&self, limit: i64) -> bool {
        let mut d = lock(&self.deletions);
        let now = Instant::now();
        while d.front().is_some_and(|t| now - *t > DELETE_WINDOW) {
            d.pop_front();
        }
        limit == 0 || (d.len() as i64) < limit
    }

    pub(super) fn limit_reported(&self) -> bool {
        let mut at = lock(&self.limit_logged);
        let now = Instant::now();
        if at.is_some_and(|t| now - t < DELETE_WINDOW) {
            return true;
        }
        *at = Some(now);
        false
    }

    fn apply_removal(&self, id: i64, x: &Entry) -> Outcome {
        let cfg = self.config();
        let full = full_name(&x.owner, &x.name);
        if let Some(why) = unwanted(&cfg, &x.owner, &x.name) {
            self.leave_unsynced(id, &full, why);
            return Outcome::Done;
        }
        let sy = &cfg.sync;
        let mut on_delete = sy.on_delete.as_str();
        if let Some(r) = cfg.repos.rule(&full)
            && !r.on_delete.is_empty()
        {
            on_delete = r.on_delete.as_str();
        }
        let name = format!("{}/{}", x.gh_owner, x.gh_name);
        match on_delete {
            "delete" => {
                if !self.within_delete_limit(sy.delete_limit) {
                    if self.limit_reported() {
                        debugf!("refusing to delete {name}: sync.delete_limit reached");
                    } else {
                        errorf!(
                            "refusing to delete {name}: {} repos were already deleted in the last hour (sync.delete_limit). If that is expected, restart to reset it.",
                            sy.delete_limit
                        );
                    }
                    return Outcome::Later;
                }
                let g = match self.tgt.get(&x.gh_owner, &x.gh_name) {
                    Ok(g) => g,
                    Err(e) => {
                        errorf!("{name}: {e}");
                        return Outcome::Later;
                    }
                };
                match g {
                    None => infof!("{name}: already gone from GitHub"),
                    Some(g) if !eq_fold(&g.owner.login, &x.gh_owner) => {
                        warnf!("{name}: owned by someone else on GitHub, not deleting");
                    }
                    Some(g) if !eq_fold(&g.name, &x.gh_name) => {
                        warnf!(
                            "{name}: renamed on GitHub to {}/{}, not deleting",
                            g.owner.login,
                            g.name
                        );
                    }
                    Some(_) => {
                        if let Err(e) = self.tgt.delete(&x.gh_owner, &x.gh_name) {
                            errorf!("{name}: {e}");
                            return Outcome::Later;
                        }
                        lock(&self.deletions).push_back(Instant::now());
                        infof!("{name}: deleted on GitHub (removed from the source)");
                    }
                }
            }
            "archive" => {
                let g = match self.tgt.get(&x.gh_owner, &x.gh_name) {
                    Ok(g) => g,
                    Err(e) => {
                        errorf!("{name}: {e}");
                        return Outcome::Later;
                    }
                };
                match g {
                    Some(g) if !eq_fold(&g.name, &x.gh_name) => {
                        warnf!(
                            "{name}: renamed on GitHub to {}/{}, not archiving",
                            g.owner.login,
                            g.name
                        );
                    }
                    Some(g) if !g.archived => {
                        let mut f = Fields::new();
                        f.insert("archived".into(), Value::Bool(true));
                        if let Err(e) = self.tgt.patch(&x.gh_owner, &x.gh_name, &f) {
                            errorf!("{name}: {e}");
                            return Outcome::Later;
                        }
                        infof!("{name}: archived on GitHub (removed from the source)");
                    }
                    _ => {}
                }
            }
            _ => {
                infof!(
                    "{name}: removed from the source, the GitHub copy is left alone (on_delete = ignore)"
                );
            }
        }
        self.state.drop_entry(id);
        self.mir.forget(id);
        self.state.save();
        Outcome::Done
    }
}
