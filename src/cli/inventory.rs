use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use super::App;
use crate::engine::{Entry, Verdict, glob_match};
use crate::error::Error;
use crate::forge::Repo;

pub(crate) const ST_SYNCED: &str = "synced";
pub(crate) const ST_ERROR: &str = "error";
pub(crate) const ST_QUEUED: &str = "queued";
pub(crate) const ST_WAITING: &str = "waiting";
pub(crate) const ST_IGNORED: &str = "ignored";
pub(crate) const ST_FILTERED: &str = "filtered";

#[derive(Debug, Clone)]
pub(crate) struct Item {
    pub(crate) repo: Arc<Repo>,
    pub(crate) verdict: Verdict,
    pub(crate) entry: Entry,
    pub(crate) has: bool,
    pub(crate) status: &'static str,
}

impl Item {
    pub(crate) fn full(&self) -> String {
        self.repo.full()
    }

    pub(crate) fn managed(&self) -> bool {
        self.has && self.entry.managed
    }

    pub(crate) fn target(&self) -> String {
        if self.has && !self.entry.gh_owner.is_empty() {
            return format!("{}/{}", self.entry.gh_owner, self.entry.gh_name);
        }
        String::new()
    }
}

pub(crate) fn classify(v: &Verdict, x: &Entry, has: bool) -> &'static str {
    if v.pending {
        ST_WAITING
    } else if v.sync && has && x.managed && !x.last_error.is_empty() {
        ST_ERROR
    } else if v.sync && has && x.managed {
        ST_SYNCED
    } else if v.sync {
        ST_QUEUED
    } else if v.rule.as_ref().is_some_and(|r| r.sync.is_some()) {
        ST_IGNORED
    } else {
        ST_FILTERED
    }
}

pub(crate) fn inventory(a: &App) -> Result<Vec<Item>, Error> {
    let repos = a.eng.list_all()?;
    let mut entries = a.eng.entries();
    let mut out: Vec<Item> = repos
        .into_iter()
        .map(|r| {
            let verdict = a.eng.decide(&r);
            let (entry, has) = match entries.remove(&r.id) {
                Some(x) => (x, true),
                None => (Entry::default(), false),
            };
            let status = classify(&verdict, &entry, has);
            Item {
                repo: r,
                verdict,
                entry,
                has,
                status,
            }
        })
        .collect();
    out.sort_by_cached_key(|it| it.full().to_lowercase());
    Ok(out)
}

#[derive(Default)]
pub(crate) struct Counts(HashMap<&'static str, usize>);

impl Counts {
    pub(crate) fn of(items: &[Item]) -> Counts {
        let mut c = Counts::default();
        for it in items {
            *c.0.entry(it.status).or_default() += 1;
        }
        c
    }

    pub(crate) fn get(&self, st: &str) -> usize {
        self.0.get(st).copied().unwrap_or(0)
    }
}

pub(crate) fn matching(items: &[Item], pattern: &str) -> Vec<usize> {
    let p = pattern.to_lowercase();
    let bare = !p.contains('/');
    items
        .iter()
        .enumerate()
        .filter(|(_, it)| {
            glob_match(&p, &it.full().to_lowercase())
                || (bare && glob_match(&p, &it.repo.name.to_lowercase()))
        })
        .map(|(i, _)| i)
        .collect()
}

pub(crate) fn resolve(items: &[Item], patterns: &[String]) -> Result<Vec<usize>, Error> {
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    for p in patterns {
        let found = matching(items, p);
        if found.is_empty() {
            return Err(Error::Config(format!(
                "no repository matches {p:?} (see `gitsync repos list`)"
            )));
        }
        for i in found {
            if seen.insert(items[i].repo.id) {
                out.push(i);
            }
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::RepoRule;

    fn item(owner: &str, name: &str) -> Item {
        Item {
            repo: Arc::new(Repo {
                id: name.len() as i64 + owner.len() as i64 * 100,
                owner: owner.into(),
                name: name.into(),
                ..Repo::default()
            }),
            verdict: Verdict::default(),
            entry: Entry::default(),
            has: false,
            status: ST_FILTERED,
        }
    }

    #[test]
    fn patterns_match_full_names_or_bare_names() {
        let items = vec![item("org", "api"), item("org", "web"), item("me", "api")];
        assert_eq!(matching(&items, "org/*"), vec![0, 1]);
        assert_eq!(matching(&items, "API"), vec![0, 2]);
        assert_eq!(matching(&items, "me/a*"), vec![2]);
        assert!(matching(&items, "x/api").is_empty());
        assert_eq!(
            resolve(&items, &["api".into(), "org/api".into()]).unwrap(),
            vec![0, 2]
        );
        let e = resolve(&items, &["nope".into()]).unwrap_err();
        assert!(e.is_config());
        assert_eq!(
            e.to_string(),
            "no repository matches \"nope\" (see `gitsync repos list`)"
        );
    }

    #[test]
    fn classify_covers_every_status() {
        let managed = Entry {
            managed: true,
            ..Entry::default()
        };
        let failing = Entry {
            managed: true,
            last_error: "boom".into(),
            ..Entry::default()
        };
        let sync = Verdict {
            sync: true,
            ..Verdict::default()
        };
        let pending = Verdict {
            pending: true,
            ..Verdict::default()
        };
        let ignored = Verdict {
            rule: Some(RepoRule {
                sync: Some(false),
                ..RepoRule::default()
            }),
            ..Verdict::default()
        };
        assert_eq!(classify(&pending, &managed, true), ST_WAITING);
        assert_eq!(classify(&sync, &failing, true), ST_ERROR);
        assert_eq!(classify(&sync, &managed, true), ST_SYNCED);
        assert_eq!(classify(&sync, &managed, false), ST_QUEUED);
        assert_eq!(classify(&ignored, &managed, true), ST_IGNORED);
        assert_eq!(classify(&Verdict::default(), &managed, true), ST_FILTERED);
    }
}
