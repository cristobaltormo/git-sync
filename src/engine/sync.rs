use std::sync::Arc;

use super::queue::Job;
use super::settings::{describe_repo, homepage};
use super::state::Entry;
use super::{Engine, unix_now, unix_now_f64};
use crate::config::{Config, RepoRule};
use crate::error::Error;
use crate::forge::Repo;
use crate::github::{Snap, snapshot};
use crate::mirror::Options;

pub(super) struct RepoSync<'a> {
    pub(super) e: &'a Engine,
    pub(super) cfg: &'a Config,
    pub(super) repo: &'a Repo,
    pub(super) job: &'a Job,
    pub(super) owner: String,
    pub(super) name: String,
    pub(super) ent: Entry,
    pub(super) has: bool,
    pub(super) managed: bool,
    pub(super) cur: Snap,
    pub(super) cached: bool,
    pub(super) actions: Vec<String>,
    pub(super) rule: RepoRule,
}

impl RepoSync<'_> {
    pub(super) fn metadata(&self) -> bool {
        self.rule.metadata.unwrap_or(self.cfg.sync.metadata)
    }

    fn tags(&self) -> bool {
        self.rule.tags.unwrap_or(self.cfg.sync.tags)
    }

    fn prune(&self) -> bool {
        self.rule.prune.unwrap_or(true)
    }

    pub(super) fn keep_private(&self) -> bool {
        self.rule.keep_private.unwrap_or(false)
    }

    pub(super) fn note(&mut self, s: String) {
        self.actions.push(s);
    }
}

impl Engine {
    pub(super) fn sync_repo(&self, repo: &Arc<Repo>, job: &Job) -> Result<(), Error> {
        let cfg = self.config();
        let Some(owner) = cfg.github_owner(&repo.owner) else {
            return Ok(());
        };
        let mut s = RepoSync {
            e: self,
            cfg: &cfg,
            repo,
            job,
            owner: owner.to_string(),
            name: repo.name.clone(),
            ent: Entry::default(),
            has: false,
            managed: false,
            cur: Snap::default(),
            cached: false,
            actions: Vec::new(),
            rule: RepoRule::default(),
        };
        if let Some(r) = cfg.repos.rule(&repo.full()) {
            s.rule = r.clone();
            if !r.name.is_empty() {
                s.name.clone_from(&r.name);
            }
        }
        if let Some(x) = self.state.get(repo.id) {
            s.ent = x;
            s.has = true;
        }
        s.managed = s.has && s.ent.managed;

        s.rename()?;
        s.resolve()?;
        self.state.update(repo.id, true, |x| {
            x.owner.clone_from(&repo.owner);
            x.name.clone_from(&repo.name);
            x.gh_owner.clone_from(&s.owner);
            x.gh_name.clone_from(&s.name);
            x.managed = true;
            x.excluded = false;
            x.missing_since = 0;
            x.pending = 0;
        });
        let res = s.apply();
        let snap = s.cur.clone();
        self.state.update(repo.id, false, |x| x.gh = Some(snap));
        if res.is_err() {
            self.state.save();
        }
        res
    }
}

impl RepoSync<'_> {
    fn apply(&mut self) -> Result<(), Error> {
        let skip = self.skip_push();
        if !skip {
            self.refresh_private()?;
        }
        self.before_push(skip)?;
        if !skip {
            self.push()?;
        }
        self.after_push(skip)?;
        if self.e.hook_mode() == "repo" {
            self.e.repo_hook(self.repo, &self.ent.hook);
        }
        self.finish();
        Ok(())
    }

    fn resolve(&mut self) -> Result<(), Error> {
        if self.managed
            && !self.job.verify
            && let Some(gh) = &self.ent.gh
        {
            self.cur = gh.clone();
            self.cached = true;
            return Ok(());
        }
        if let Some(data) = self.e.tgt.get(&self.owner, &self.name)? {
            self.cur = snapshot(&data);
            if !self.managed {
                if !self.cfg.sync.adopt_existing {
                    return Err(Error::Blocked(format!(
                        "{}/{} already exists on GitHub and gitsync did not create it; set sync.adopt_existing = true to take it over",
                        self.owner, self.name
                    )));
                }
                self.note("adopted existing GitHub repo".into());
            }
            return Ok(());
        }
        let (desc, home) = if self.metadata() {
            (describe_repo(self.repo).to_string(), homepage(self.repo))
        } else {
            (String::new(), String::new())
        };
        let created = self.e.tgt.create(&self.owner, &self.name, &desc, &home)?;
        self.cur = match created {
            Some(r) => snapshot(&r),
            None => Snap {
                private: true,
                description: desc,
                homepage: home,
                ..Snap::default()
            },
        };
        self.note("created on GitHub (private)".into());
        Ok(())
    }

    fn refresh_private(&mut self) -> Result<(), Error> {
        if !self.cached || !self.repo.private || !self.cfg.sync.visibility {
            return Ok(());
        }
        if let Some(data) = self.e.tgt.get(&self.owner, &self.name)? {
            self.cur = snapshot(&data);
            self.cached = false;
        }
        Ok(())
    }

    fn skip_push(&self) -> bool {
        let sy = &self.cfg.sync;
        let unchanged = self.managed
            && !self.repo.updated.is_empty()
            && self.ent.updated == self.repo.updated
            && self.ent.last_error.is_empty();
        let recent =
            self.ent.last_push > 0 && unix_now_f64() - (self.ent.last_push as f64) < 86400.0;
        self.repo.empty
            || (self.cur.archived && !sy.archived)
            || (self.cur.archived && self.repo.archived && unchanged)
            || (self.job.verify && !self.job.force && unchanged && recent)
    }

    fn push(&mut self) -> Result<(), Error> {
        let last = if !self.job.verify && !self.job.force {
            self.ent.refs.as_str()
        } else {
            ""
        };
        let opt = Options {
            tags: self.tags(),
            prune: self.prune(),
        };
        let res = self
            .e
            .mir
            .mirror(self.repo, &self.owner, &self.name, last, opt)?;
        let now = unix_now();
        let summary = res.summary;
        let refs = res.refs;
        self.e.state.update(self.repo.id, false, |x| {
            x.last_push = now;
            x.refs = refs;
        });
        if summary != "up to date" {
            self.actions.insert(0, format!("pushed ({summary})"));
        }
        Ok(())
    }

    fn finish(&mut self) {
        let now = unix_now();
        let snap = self.cur.clone();
        let sig = self.repo.sig();
        self.e.state.update(self.repo.id, false, |x| {
            x.sig = sig;
            x.updated.clone_from(&self.repo.updated);
            x.gh = Some(snap);
            x.last_ok = now;
            x.attempts = 0;
            x.last_error.clear();
            x.retry_at = 0;
        });
        self.e.state.save();
        let msg = if self.actions.is_empty() {
            "nothing to do".to_string()
        } else {
            self.actions.join(", ")
        };
        infof!("{}: {}: {}", self.repo.full(), self.job.why, msg);
    }
}
