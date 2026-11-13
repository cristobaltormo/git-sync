use std::collections::HashSet;
use std::sync::Arc;

use super::notify::{EVENT_PENDING, Event};
use super::queue::Job;
use super::state::Entry;
use super::{Engine, eq_fold, unix_now};
use crate::config::Config;
use crate::error::Error;
use crate::forge::Repo;

impl Engine {
    pub fn list_all(&self) -> Result<Vec<Arc<Repo>>, Error> {
        let cfg = self.config();
        let mut all: Vec<Arc<Repo>> = Vec::new();
        for owner in cfg.accounts.keys() {
            let repos = self.src.list(owner)?;
            if all.is_empty() {
                all = repos;
            } else {
                all.extend(repos);
            }
        }
        Ok(all)
    }

    pub fn reconcile(&self, verify: bool, why: &'static str) -> bool {
        match self.list_all() {
            Ok(repos) => {
                self.reconcile_repos(&repos, verify, why);
                true
            }
            Err(e) => {
                errorf!("listing repos failed, skipping this scan: {e}");
                false
            }
        }
    }

    pub(super) fn reconcile_repos(&self, repos: &[Arc<Repo>], verify: bool, why: &'static str) {
        let cfg = self.config();
        let now = unix_now();
        let mut seen: HashSet<i64> = HashSet::with_capacity(repos.len());
        for repo in repos {
            seen.insert(repo.id);
            let ent = self
                .state
                .with(|st| st.repos.get(&repo.id).map(|x| Glance::of(x, repo)));
            let v = self.decide_in(&cfg, repo);
            if v.pending {
                self.mark_pending(repo, ent.as_ref());
                continue;
            }
            if ent.as_ref().is_some_and(|x| x.pending) {
                self.state.update(repo.id, false, |x| x.pending = 0);
                self.state.save();
            }
            if !v.sync {
                if ent.as_ref().is_some_and(|x| x.managed && !x.excluded) {
                    self.enqueue(Job {
                        key: repo.id.to_string(),
                        repo: Some(repo.clone()),
                        why,
                        ..Job::default()
                    });
                }
                continue;
            }
            if ent.as_ref().is_some_and(|x| x.retry_at > now) {
                continue;
            }
            let due = ent
                .as_ref()
                .is_none_or(|x| !x.managed || x.excluded || x.failed || !x.same);
            if due || verify {
                self.enqueue(Job {
                    key: repo.id.to_string(),
                    repo: Some(repo.clone()),
                    why: if due { why } else { "verify" },
                    verify,
                    ..Job::default()
                });
            }
        }
        let (managed, missing) = self.state.with(|st| {
            let managed = st.repos.values().filter(|x| x.managed).count();
            let missing: Vec<(i64, Entry)> = st
                .repos
                .iter()
                .filter(|(id, x)| !seen.contains(id) && !x.excluded)
                .map(|(id, x)| (*id, x.clone()))
                .collect();
            (managed, missing)
        });
        if missing.is_empty() {
            return;
        }
        if repos.is_empty() && managed > 1 {
            warnf!("the source returned no repos at all; not treating that as a mass deletion");
            return;
        }
        let silent = self.silent_owners(&cfg, repos);
        for (id, x) in missing {
            if silent.iter().any(|o| eq_fold(o, &x.owner)) {
                continue;
            }
            self.handle_missing(id, x);
        }
    }

    fn silent_owners(&self, cfg: &Config, repos: &[Arc<Repo>]) -> Vec<String> {
        let mut out = Vec::new();
        for owner in cfg.accounts.keys() {
            if repos.iter().any(|r| eq_fold(&r.owner, owner)) {
                continue;
            }
            let managed = self.state.with(|st| {
                st.repos
                    .values()
                    .filter(|x| x.managed && eq_fold(&x.owner, owner))
                    .count()
            });
            if managed > 1 {
                warnf!(
                    "the source returned no repos for {owner}; not treating that as a mass deletion (did the token lose access?)"
                );
                out.push(owner.clone());
            }
        }
        out
    }

    fn mark_pending(&self, repo: &Repo, ent: Option<&Glance>) {
        if ent.is_some_and(|x| x.pending) {
            return;
        }
        let now = unix_now();
        self.state.update(repo.id, true, |x| {
            x.owner.clone_from(&repo.owner);
            x.name.clone_from(&repo.name);
            x.pending = now;
        });
        self.state.save();
        let full = repo.full();
        infof!("{full}: new repository, not synced until you decide (run `gitsync repos`)");
        self.notify(Event {
            kind: EVENT_PENDING,
            repo: full,
            info: String::new(),
        });
    }

    pub(super) fn next_retry(&self, after: i64) -> Option<i64> {
        self.state.with(|st| {
            st.repos
                .values()
                .filter(|x| !x.last_error.is_empty() && x.retry_at > 0 && x.retry_at > after)
                .map(|x| x.retry_at)
                .min()
        })
    }

    pub fn has_missing(&self) -> bool {
        self.state
            .with(|st| st.repos.values().any(|x| x.missing_since != 0))
    }
}

struct Glance {
    managed: bool,
    excluded: bool,
    pending: bool,
    failed: bool,
    retry_at: i64,
    same: bool,
}

impl Glance {
    fn of(x: &Entry, repo: &Repo) -> Glance {
        Glance {
            managed: x.managed,
            excluded: x.excluded,
            pending: x.pending != 0,
            failed: !x.last_error.is_empty(),
            retry_at: x.retry_at,
            same: x.updated == repo.updated && x.sig == repo.sig(),
        }
    }
}
