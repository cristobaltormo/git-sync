use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::io::Write as _;
use std::path::Path;

use serde::Deserialize;

use super::owner::{self, TempFile};
use super::{
    Config, fold_keys, join, one_of, parent_dir, parse_toml, quote_key, quote_string, read_text,
    report_unknown, unknown_keys,
};
use crate::config_err;
use crate::error::{Error, io_message};

#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(default)]
pub struct RepoRule {
    pub sync: Option<bool>,
    pub name: String,
    pub keep_private: Option<bool>,
    pub tags: Option<bool>,
    pub metadata: Option<bool>,
    pub prune: Option<bool>,
    pub on_delete: String,
    pub note: String,
}

pub(super) const RULE_FIELDS: &[&str] = &[
    "sync",
    "name",
    "keep_private",
    "tags",
    "metadata",
    "prune",
    "on_delete",
    "note",
];

impl RepoRule {
    pub fn is_empty(&self) -> bool {
        *self == RepoRule::default()
    }

    fn validate(&self, key: &str) -> Result<(), Error> {
        if !self.name.is_empty() && !is_github_name(&self.name) {
            return Err(config_err!(
                "repos.{key:?}: name {:?} is not a valid GitHub repository name",
                self.name
            ));
        }
        if !self.on_delete.is_empty() && !one_of(&self.on_delete, &["delete", "archive", "ignore"])
        {
            return Err(config_err!(
                "repos.{key:?}: on_delete must be delete, archive or ignore"
            ));
        }
        Ok(())
    }
}

pub fn is_github_name(s: &str) -> bool {
    (1..=100).contains(&s.len())
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'))
}

#[derive(Debug, Clone, Default)]
pub struct Repos {
    pub path: String,
    pub rules: BTreeMap<String, RepoRule>,
}

pub fn repos_path(cfg: &Config) -> String {
    if !cfg.paths.repos_file.is_empty() {
        return cfg.paths.repos_file.clone();
    }
    if !cfg.path.is_empty() {
        return join(&parent_dir(&cfg.path), "repos.toml");
    }
    join(&cfg.paths.state_dir, "repos.toml")
}

#[derive(Deserialize)]
struct File {
    #[serde(default)]
    repos: BTreeMap<String, RepoRule>,
}

impl Repos {
    pub fn load(path: &str) -> Result<Repos, Error> {
        let mut r = Repos {
            path: path.to_string(),
            rules: BTreeMap::new(),
        };
        let Some(text) = read_text(path)? else {
            return Ok(r);
        };
        let mut table = parse_toml(path, &text)?;
        fold_keys(&mut table, |k| {
            k.eq_ignore_ascii_case("repos").then_some("repos")
        });
        if let Some(toml::Value::Table(rules)) = table.get_mut("repos") {
            for (_, rule) in rules.iter_mut() {
                if let toml::Value::Table(fields) = rule {
                    fold_keys(fields, |f| {
                        RULE_FIELDS
                            .iter()
                            .copied()
                            .find(|r| r.eq_ignore_ascii_case(f))
                    });
                }
            }
        }
        let mut unknown = Vec::new();
        for (k, v) in &table {
            if k == "repos" {
                if let toml::Value::Table(rules) = v {
                    for (rk, rv) in rules {
                        let toml::Value::Table(fields) = rv else {
                            continue;
                        };
                        for (fk, fv) in fields {
                            if RULE_FIELDS.contains(&fk.as_str()) {
                                continue;
                            }
                            let name = format!("repos.{}.{}", quote_key(rk), quote_key(fk));
                            if let toml::Value::Table(inner) = fv {
                                unknown_keys(&name, inner, &mut unknown);
                            }
                            unknown.push(name);
                        }
                    }
                }
                continue;
            }
            let name = quote_key(k);
            if let toml::Value::Table(inner) = v {
                unknown_keys(&name, inner, &mut unknown);
            }
            unknown.push(name);
        }
        report_unknown(path, unknown)?;
        let file: File = toml::Value::Table(table)
            .try_into()
            .map_err(|e: toml::de::Error| config_err!("{path}: {}", e.to_string().trim_end()))?;
        for (k, rule) in file.repos {
            rule.validate(&k).map_err(|e| config_err!("{path}: {e}"))?;
            r.rules.insert(k.to_lowercase(), rule);
        }
        Ok(r)
    }

    pub fn rule(&self, full: &str) -> Option<&RepoRule> {
        if self.rules.is_empty() {
            return None;
        }
        if full.bytes().any(|b| b.is_ascii_uppercase()) || !full.is_ascii() {
            return self.rules.get(&full.to_lowercase());
        }
        self.rules.get(full)
    }

    pub fn edit(&mut self, full: &str, f: impl FnOnce(&mut RepoRule)) -> Result<(), Error> {
        let key = full.to_lowercase();
        let mut rule = self.rules.get(&key).cloned().unwrap_or_default();
        f(&mut rule);
        rule.validate(full)?;
        if rule.is_empty() {
            self.rules.remove(&key);
        } else {
            self.rules.insert(key, rule);
        }
        Ok(())
    }

    pub fn render(&self) -> String {
        let mut out = String::from(REPOS_HEADER);
        for (i, (key, r)) in self.rules.iter().enumerate() {
            if i > 0 {
                out.push('\n');
            }
            let _ = writeln!(out, "[repos.{}]", quote_key(key));
            flag(&mut out, "sync", r.sync);
            text(&mut out, "name", &r.name);
            flag(&mut out, "keep_private", r.keep_private);
            flag(&mut out, "tags", r.tags);
            flag(&mut out, "metadata", r.metadata);
            flag(&mut out, "prune", r.prune);
            text(&mut out, "on_delete", &r.on_delete);
            text(&mut out, "note", &r.note);
        }
        out
    }

    pub fn save(&self) -> Result<(), Error> {
        let dir = parent_dir(&self.path);
        std::fs::create_dir_all(&dir)
            .map_err(|e| config_err!("cannot create {dir}: {}", io_message(&e)))?;
        let (tmp, mut file) = TempFile::create(&dir, ".repos-", ".toml")
            .map_err(|e| config_err!("cannot write {}: {}", self.path, io_message(&e)))?;
        file.write_all(self.render().as_bytes())?;
        drop(file);
        let prev = std::fs::metadata(&self.path).ok();
        let mode = prev.as_ref().map_or(0o644, owner::mode);
        owner::set_mode(tmp.path(), mode)?;
        match &prev {
            Some(prev) => owner::keep_owner(tmp.path(), prev),
            None => owner::like_dir(tmp.path(), Path::new(&dir)),
        }
        tmp.persist(&self.path)?;
        Ok(())
    }
}

fn flag(out: &mut String, field: &str, v: Option<bool>) {
    if let Some(v) = v {
        let _ = writeln!(out, "{field} = {v}");
    }
}

fn text(out: &mut String, field: &str, v: &str) {
    if !v.is_empty() {
        let _ = writeln!(out, "{field} = {}", quote_string(v));
    }
}

const REPOS_HEADER: &str = r#"# Per-repository decisions and overrides, written by "gitsync repos".
# Safe to edit by hand; a running gitsync picks changes up on its own.
#
#   sync         true: always sync it, false: never sync it (beats every filter)
#   name         repository name to use on GitHub
#   keep_private never make the GitHub copy public
#   tags         mirror tags (overrides sync.tags)
#   metadata     copy description and topics (overrides sync.metadata)
#   prune        false: keep branches that exist only on GitHub (pull request heads)
#   on_delete    delete, archive or ignore when it disappears from the source
#   note         free text for you

"#;
