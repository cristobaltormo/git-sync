use std::time::{Duration, Instant};

use super::filter::unwanted;
use super::queue::Job;
use super::{Engine, eq_fold, full_name, lock};
use crate::forge::{Change, Event, Repo};

pub const HOOK_DELAY: Duration = Duration::from_millis(100);

impl Engine {
    pub fn hook_mode(&self) -> &'static str {
        *lock(&self.hook_mode)
    }

    fn set_hook_mode(&self, m: &'static str) {
        *lock(&self.hook_mode) = m;
    }

    pub fn setup_hooks(&self, force: bool) -> bool {
        let cfg = self.config();
        let mode = cfg.hooks.mode.as_str();
        let h = match self.src.hooks() {
            Some(h) if mode != "none" && !self.dry => h,
            _ => {
                self.set_hook_mode("none");
                return true;
            }
        };
        let me = match self.src.whoami() {
            Ok(me) => me,
            Err(e) => {
                errorf!("cannot set up webhooks yet, the source is not reachable: {e}");
                return false;
            }
        };
        if mode == "system" || (mode == "auto" && me.system_hooks) {
            let known = self.system_hook();
            match h.global(known, force) {
                Ok((id, res)) => {
                    self.state.set_system_hook(id);
                    self.set_hook_mode("system");
                    infof!("system webhook {res} (covers every repo, new ones included)");
                    return true;
                }
                Err(e) if mode == "system" => {
                    errorf!("cannot create the system webhook: {e}");
                    return false;
                }
                Err(e) => {
                    warnf!("system webhook failed ({e}), falling back to per-repo webhooks");
                }
            }
        }
        self.set_hook_mode("repo");
        infof!("using per-repo webhooks (created as each repo is synced)");
        true
    }

    pub fn handle_event(&self, ev: &Event) {
        if self.config().github_owner(&ev.owner).is_none() {
            return;
        }
        if ev.kind == Change::Removed {
            self.wake();
            return;
        }
        let key = if ev.id == 0 {
            match self.unkeyed(ev) {
                Some(k) => k,
                None => return,
            }
        } else {
            ev.id.to_string()
        };
        self.enqueue(Job {
            key,
            owner: ev.owner.clone(),
            name: ev.name.clone(),
            why: "webhook",
            ready_at: Some(Instant::now() + HOOK_DELAY),
            ..Job::default()
        });
    }

    fn unkeyed(&self, ev: &Event) -> Option<String> {
        let full = full_name(&ev.owner, &ev.name);
        if unwanted(&self.config(), &ev.owner, &ev.name).is_some() {
            debugf!("{full}: webhook ignored, the repo is not synced");
            return None;
        }
        let known = self.state.with(|st| {
            st.repos
                .iter()
                .find(|(_, x)| eq_fold(&x.owner, &ev.owner) && eq_fold(&x.name, &ev.name))
                .map(|(id, _)| *id)
        });
        Some(known.map_or(full, |id| id.to_string()))
    }

    pub(super) fn repo_hook(&self, repo: &Repo, have: &str) {
        let url = self.config().hook_url();
        let Some(h) = self.src.hooks() else { return };
        if have == url || self.dry {
            return;
        }
        if let Err(e) = h.repo(repo, false) {
            warnf!("{}: could not create the webhook: {e}", repo.full());
            return;
        }
        self.state.update(repo.id, false, |x| x.hook = url);
    }
}
