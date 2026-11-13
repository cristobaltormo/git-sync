use std::collections::{BTreeSet, HashMap, HashSet};

use super::notify::{EVENT_PULL_REQUEST, Event};
use super::{Engine, full_name};
use crate::config::Config;

pub const DEFAULT_PULL_MESSAGE: &str = "This repository is a read-only mirror. Whatever is merged here is overwritten by the next sync, so this pull request cannot be merged on GitHub. Please send it to the maintainer instead.";

impl Engine {
    pub(super) fn check_pulls(&self) {
        let cfg = self.config();
        let comment = cfg.pulls.mode == "comment";
        let tell =
            !cfg.notify.url.is_empty() && cfg.notify.events.iter().any(|e| e == EVENT_PULL_REQUEST);
        if (!comment && !tell) || self.dry {
            return;
        }
        let (managed, known, ready) = self.state.with(|st| {
            let managed: HashMap<String, String> = st
                .repos
                .values()
                .filter(|x| x.managed && !x.gh_owner.is_empty())
                .map(|x| {
                    (
                        format!("{}/{}", x.gh_owner, x.gh_name).to_lowercase(),
                        full_name(&x.owner, &x.name),
                    )
                })
                .collect();
            let known: HashSet<String> = st.pulls.keys().cloned().collect();
            (managed, known, st.pulls_ready)
        });
        let owners: BTreeSet<&str> = cfg.accounts.values().map(String::as_str).collect();
        let mut open: HashSet<String> = HashSet::new();
        let mut complete = true;
        for owner in owners {
            let pulls = match self.tgt.open_pulls(owner) {
                Ok(p) => p,
                Err(e) => {
                    warnf!("cannot look for pull requests of {owner}: {e}");
                    complete = false;
                    continue;
                }
            };
            for p in pulls {
                let Some(src) = managed.get(&format!("{}/{}", p.owner, p.repo).to_lowercase())
                else {
                    continue;
                };
                let key = p.key();
                self.state.mark_pull(&key);
                let seen = known.contains(&key);
                open.insert(key.clone());
                if seen || !ready {
                    continue;
                }
                if comment {
                    match self
                        .tgt
                        .comment(&p.owner, &p.repo, p.number, &pull_message(&cfg, src))
                    {
                        Err(e) => warnf!("{key}: could not comment: {e}"),
                        Ok(()) => infof!(
                            "{key}: pull request on a mirror, left a note (it is never merged or closed by gitsync)"
                        ),
                    }
                }
                self.notify(Event {
                    kind: EVENT_PULL_REQUEST,
                    repo: src.clone(),
                    info: p.url.clone(),
                });
            }
        }
        self.state.finish_pulls(&open, complete);
        self.state.save();
    }
}

pub(super) fn pull_message(cfg: &Config, src: &str) -> String {
    let msg = &cfg.pulls.message;
    if msg.is_empty() {
        return DEFAULT_PULL_MESSAGE.to_string();
    }
    let url = format!("{}/{}", cfg.source_url(), src);
    let mut out = String::with_capacity(msg.len() + url.len());
    let mut rest = msg.as_str();
    while !rest.is_empty() {
        if let Some(r) = rest.strip_prefix("{repo}") {
            out.push_str(src);
            rest = r;
        } else if let Some(r) = rest.strip_prefix("{url}") {
            out.push_str(&url);
            rest = r;
        } else {
            let c = rest.chars().next().unwrap_or_default();
            out.push(c);
            rest = &rest[c.len_utf8()..];
        }
    }
    out
}
