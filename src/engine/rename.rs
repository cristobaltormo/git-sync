use serde_json::Value;

use super::eq_fold;
use super::state::Entry;
use super::sync::RepoSync;
use crate::error::Error;
use crate::github::Fields;

impl RepoSync<'_> {
    pub(super) fn rename(&mut self) -> Result<(), Error> {
        if !self.managed || (self.ent.gh_owner == self.owner && self.ent.gh_name == self.name) {
            return Ok(());
        }
        if !eq_fold(&self.ent.gh_owner, &self.owner) {
            warnf!(
                "{}: moved to another account; the old GitHub copy {}/{} is left as it is",
                self.repo.full(),
                self.ent.gh_owner,
                self.ent.gh_name
            );
            self.managed = false;
            self.has = false;
            self.ent = Entry::default();
            return Ok(());
        }
        let old = self.e.tgt.get(&self.ent.gh_owner, &self.ent.gh_name)?;
        if old.is_some() {
            if !eq_fold(&self.ent.gh_name, &self.name)
                && self.e.tgt.get(&self.owner, &self.name)?.is_some()
            {
                return Err(Error::Blocked(format!(
                    "cannot rename {}/{} to {}: that name already exists on GitHub",
                    self.ent.gh_owner, self.ent.gh_name, self.name
                )));
            }
            let mut f = Fields::new();
            f.insert("name".into(), Value::String(self.name.clone()));
            self.e
                .tgt
                .patch(&self.ent.gh_owner, &self.ent.gh_name, &f)?;
            self.note(format!("renamed {} -> {}", self.ent.gh_name, self.name));
        }
        let gone = old.is_none();
        let name = self.name.clone();
        self.e.state.update(self.repo.id, false, |x| {
            x.gh_name = name;
            if gone {
                x.gh = None;
            }
        });
        self.ent.gh_name.clone_from(&self.name);
        if gone {
            self.ent.gh = None;
        }
        Ok(())
    }
}
