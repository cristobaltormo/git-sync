use serde_json::Value;

use super::sync::RepoSync;
use crate::config::is_http_url;
use crate::error::Error;
use crate::forge::Repo;
use crate::github::{Fields, Snap, clean_topics};

impl RepoSync<'_> {
    pub(super) fn before_push(&mut self, skip: bool) -> Result<(), Error> {
        let mut pre = Fields::new();
        if self.cur.archived && !skip {
            pre.insert("archived".into(), Value::Bool(false));
        }
        if self.cfg.sync.visibility && self.repo.private && !self.cur.private {
            pre.insert("private".into(), Value::Bool(true));
        }
        if pre.is_empty() {
            return Ok(());
        }
        self.e.tgt.patch(&self.owner, &self.name, &pre)?;
        apply(&mut self.cur, &pre);
        self.note(format!("set {}", join(&pre)));
        Ok(())
    }

    pub(super) fn after_push(&mut self, skipped: bool) -> Result<(), Error> {
        let sy = &self.cfg.sync;
        let mut post = Fields::new();
        if sy.visibility && !self.keep_private() && !self.repo.private && self.cur.private {
            post.insert("private".into(), Value::Bool(false));
        }
        if self.metadata() {
            let d = describe_repo(self.repo);
            if d != self.cur.description {
                post.insert("description".into(), Value::String(d.to_string()));
            }
            let h = homepage(self.repo);
            if h != self.cur.homepage {
                post.insert("homepage".into(), Value::String(h));
            }
        }
        let branch = &self.repo.default_branch;
        if sy.default_branch
            && !branch.is_empty()
            && !self.repo.empty
            && self.cur.default_branch != *branch
            && self.ent.refused_branch != *branch
        {
            post.insert("default_branch".into(), Value::String(branch.clone()));
        }
        if !post.is_empty() {
            self.patch_post(post)?;
        }
        if self.metadata() {
            let mut topics = clean_topics(&self.repo.topics, &self.cfg.filter.topic);
            topics.sort();
            if topics != self.cur.topics {
                self.e.tgt.set_topics(&self.owner, &self.name, &topics)?;
                self.cur.topics = topics;
                self.note("updated topics".into());
            }
        }
        let archived = self.repo.archived;
        if self.cfg.sync.archived && archived != self.cur.archived && (archived || skipped) {
            let mut f = Fields::new();
            f.insert("archived".into(), Value::Bool(archived));
            self.e.tgt.patch(&self.owner, &self.name, &f)?;
            self.cur.archived = archived;
            self.note(if archived { "archived" } else { "unarchived" }.into());
        }
        Ok(())
    }

    fn patch_post(&mut self, mut post: Fields) -> Result<(), Error> {
        let mut res = self.e.tgt.patch(&self.owner, &self.name, &post);
        if let Err(Error::Api { status: 422, msg }) = &res
            && post.contains_key("default_branch")
            && branch_refused(msg)
        {
            let branch = self.repo.default_branch.clone();
            warnf!(
                "{}: GitHub refused the default branch '{}', it will not be tried again",
                self.repo.full(),
                branch
            );
            self.ent.refused_branch.clone_from(&branch);
            self.e
                .state
                .update(self.repo.id, false, |x| x.refused_branch = branch);
            post.remove("default_branch");
            res = Ok(());
            if !post.is_empty() {
                res = self.e.tgt.patch(&self.owner, &self.name, &post);
            }
        }
        res?;
        apply(&mut self.cur, &post);
        if !post.is_empty() {
            let mut keys: Vec<&str> = post.keys().map(String::as_str).collect();
            keys.sort_unstable();
            self.note(format!("updated {}", keys.join(", ")));
        }
        Ok(())
    }
}

pub(super) fn branch_refused(msg: &str) -> bool {
    let m = msg.to_lowercase();
    m.contains("default_branch") || m.contains("default branch")
}

pub(super) fn describe_repo(r: &Repo) -> std::borrow::Cow<'_, str> {
    let d = &r.description;
    if d.len() <= 350 {
        return std::borrow::Cow::Borrowed(d);
    }
    let mut end = 347;
    while !d.is_char_boundary(end) {
        end -= 1;
    }
    std::borrow::Cow::Owned(format!("{}...", &d[..end]))
}

pub(super) fn homepage(r: &Repo) -> String {
    if is_http_url(&r.website) {
        r.website.clone()
    } else {
        String::new()
    }
}

fn text(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}

fn join(fields: &Fields) -> String {
    let mut parts: Vec<String> = fields
        .iter()
        .map(|(k, v)| format!("{k}={}", text(v)))
        .collect();
    parts.sort();
    parts.join(", ")
}

fn apply(s: &mut Snap, fields: &Fields) {
    for (k, v) in fields {
        match (k.as_str(), v) {
            ("private", Value::Bool(b)) => s.private = *b,
            ("archived", Value::Bool(b)) => s.archived = *b,
            ("description", Value::String(t)) => s.description.clone_from(t),
            ("homepage", Value::String(t)) => s.homepage.clone_from(t),
            ("default_branch", Value::String(t)) => s.default_branch.clone_from(t),
            _ => {}
        }
    }
}

#[cfg(test)]
pub(super) fn join_fields(fields: &Fields) -> String {
    join(fields)
}
