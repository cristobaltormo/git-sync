use super::Config;
use crate::config_err;
use crate::error::Error;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Str,
    Bool,
    Int,
    List,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Key {
    pub name: &'static str,
    pub kind: Kind,
    pub secret: bool,
}

const fn key(name: &'static str, kind: Kind) -> Key {
    Key {
        name,
        kind,
        secret: false,
    }
}

const fn secret(name: &'static str) -> Key {
    Key {
        name,
        kind: Kind::Str,
        secret: true,
    }
}

use Kind::{Bool, Int, List, Str};

static KEYS: &[Key] = &[
    key("source.type", Str),
    key("source.url", Str),
    secret("source.token"),
    key("source.token_file", Str),
    key("source.user", Str),
    secret("github.token"),
    key("github.token_file", Str),
    key("github.api_url", Str),
    key("github.git_url", Str),
    key("listen.host", Str),
    key("listen.port", Int),
    secret("listen.secret"),
    key("listen.secret_file", Str),
    key("listen.public_url", Str),
    key("sync.poll_interval", Int),
    key("sync.verify_interval", Int),
    key("sync.workers", Int),
    key("sync.visibility", Bool),
    key("sync.metadata", Bool),
    key("sync.default_branch", Bool),
    key("sync.archived", Bool),
    key("sync.tags", Bool),
    key("sync.on_delete", Str),
    key("sync.delete_grace", Int),
    key("sync.delete_limit", Int),
    key("sync.adopt_existing", Bool),
    key("sync.dry_run", Bool),
    key("sync.git_timeout", Int),
    key("filter.include", List),
    key("filter.exclude", List),
    key("filter.topic", Str),
    key("filter.skip_forks", Bool),
    key("filter.skip_mirrors", Bool),
    key("filter.skip_archived", Bool),
    key("filter.skip_private", Bool),
    key("filter.scope", Str),
    key("filter.new_repos", Str),
    key("hooks.mode", Str),
    key("paths.state_dir", Str),
    key("paths.repos_file", Str),
    key("log.level", Str),
    key("notify.url", Str),
    key("notify.format", Str),
    key("notify.events", List),
    key("pull_requests.mode", Str),
    key("pull_requests.message", Str),
];

pub fn keys() -> &'static [Key] {
    KEYS
}

pub(super) fn is_section(sec: &str) -> bool {
    KEYS.iter()
        .any(|k| k.name.split_once('.').is_some_and(|(s, _)| s == sec))
}

pub(super) fn canonical_section(sec: &str) -> Option<&'static str> {
    if sec.eq_ignore_ascii_case("accounts") {
        return Some("accounts");
    }
    KEYS.iter()
        .filter_map(|k| k.name.split_once('.'))
        .map(|(s, _)| s)
        .find(|s| s.eq_ignore_ascii_case(sec))
}

pub(super) fn canonical_field(sec: &str, field: &str) -> Option<&'static str> {
    KEYS.iter()
        .filter_map(|k| k.name.split_once('.'))
        .find(|(s, f)| *s == sec && f.eq_ignore_ascii_case(field))
        .map(|(_, f)| f)
}

pub(super) fn is_field_name(name: &str) -> bool {
    KEYS.iter()
        .filter_map(|k| k.name.split_once('.'))
        .any(|(s, f)| s == name || f == name)
}

pub(super) fn is_known(sec: &str, field: &str) -> bool {
    KEYS.iter()
        .any(|k| k.name.split_once('.') == Some((sec, field)))
}

#[cfg(test)]
pub(super) fn has_value(name: &str) -> bool {
    value(&Config::default(), name).is_some()
}

pub fn find_key(name: &str) -> Option<Key> {
    let name = name.trim().to_lowercase();
    KEYS.iter().find(|k| k.name == name).copied()
}

enum Val<'a> {
    Str(&'a str),
    Bool(bool),
    Int(i64),
    List(&'a [String]),
}

fn value<'a>(c: &'a Config, name: &str) -> Option<Val<'a>> {
    let (s, f, sy, l) = (&c.source, &c.filter, &c.sync, &c.listen);
    Some(match name {
        "source.type" => Val::Str(&s.kind),
        "source.url" => Val::Str(&s.url),
        "source.token" => Val::Str(&s.token),
        "source.token_file" => Val::Str(&s.token_file),
        "source.user" => Val::Str(&s.user),
        "github.token" => Val::Str(&c.github.token),
        "github.token_file" => Val::Str(&c.github.token_file),
        "github.api_url" => Val::Str(&c.github.api_url),
        "github.git_url" => Val::Str(&c.github.git_url),
        "listen.host" => Val::Str(&l.host),
        "listen.port" => Val::Int(l.port),
        "listen.secret" => Val::Str(&l.secret),
        "listen.secret_file" => Val::Str(&l.secret_file),
        "listen.public_url" => Val::Str(&l.public_url),
        "sync.poll_interval" => Val::Int(sy.poll_interval),
        "sync.verify_interval" => Val::Int(sy.verify_interval),
        "sync.workers" => Val::Int(sy.workers),
        "sync.visibility" => Val::Bool(sy.visibility),
        "sync.metadata" => Val::Bool(sy.metadata),
        "sync.default_branch" => Val::Bool(sy.default_branch),
        "sync.archived" => Val::Bool(sy.archived),
        "sync.tags" => Val::Bool(sy.tags),
        "sync.on_delete" => Val::Str(&sy.on_delete),
        "sync.delete_grace" => Val::Int(sy.delete_grace),
        "sync.delete_limit" => Val::Int(sy.delete_limit),
        "sync.adopt_existing" => Val::Bool(sy.adopt_existing),
        "sync.dry_run" => Val::Bool(sy.dry_run),
        "sync.git_timeout" => Val::Int(sy.git_timeout),
        "filter.include" => Val::List(&f.include),
        "filter.exclude" => Val::List(&f.exclude),
        "filter.topic" => Val::Str(&f.topic),
        "filter.skip_forks" => Val::Bool(f.skip_forks),
        "filter.skip_mirrors" => Val::Bool(f.skip_mirrors),
        "filter.skip_archived" => Val::Bool(f.skip_archived),
        "filter.skip_private" => Val::Bool(f.skip_private),
        "filter.scope" => Val::Str(&f.scope),
        "filter.new_repos" => Val::Str(&f.new_repos),
        "hooks.mode" => Val::Str(&c.hooks.mode),
        "paths.state_dir" => Val::Str(&c.paths.state_dir),
        "paths.repos_file" => Val::Str(&c.paths.repos_file),
        "log.level" => Val::Str(&c.log.level),
        "notify.url" => Val::Str(&c.notify.url),
        "notify.format" => Val::Str(&c.notify.format),
        "notify.events" => Val::List(&c.notify.events),
        "pull_requests.mode" => Val::Str(&c.pulls.mode),
        "pull_requests.message" => Val::Str(&c.pulls.message),
        _ => return None,
    })
}

impl Key {
    pub fn get(&self, c: &Config) -> String {
        match value(c, self.name).unwrap_or(Val::Str("")) {
            Val::Str(s) => s.to_string(),
            Val::Bool(b) => b.to_string(),
            Val::Int(n) => n.to_string(),
            Val::List(l) => l.join(", "),
        }
    }

    pub fn hint(&self) -> &'static str {
        match self.kind {
            Kind::Bool => "true or false",
            Kind::Int => "a number",
            Kind::List => "a comma separated list",
            Kind::Str => "text",
        }
    }

    pub fn literal<S: AsRef<str>>(&self, vals: &[S]) -> Result<String, Error> {
        let name = self.name;
        match self.kind {
            Kind::Bool => {
                let [v] = vals else {
                    return Err(config_err!("{name} takes one value (true or false)"));
                };
                match v.as_ref().to_lowercase().as_str() {
                    "true" | "yes" | "on" | "1" => Ok("true".into()),
                    "false" | "no" | "off" | "0" => Ok("false".into()),
                    _ => Err(config_err!("{name} must be true or false")),
                }
            }
            Kind::Int => {
                let [v] = vals else {
                    return Err(config_err!("{name} takes one number"));
                };
                let n: i64 =
                    parse_int(v.as_ref()).ok_or_else(|| config_err!("{name} must be a number"))?;
                Ok(n.to_string())
            }
            Kind::List => {
                let items: Vec<String> = vals
                    .iter()
                    .flat_map(|v| v.as_ref().split(','))
                    .map(str::trim)
                    .filter(|p| !p.is_empty())
                    .map(json_string)
                    .collect();
                Ok(format!("[{}]", items.join(", ")))
            }
            Kind::Str => {
                let [v] = vals else {
                    return Err(config_err!(
                        "{name} takes one value (quote it if it has spaces)"
                    ));
                };
                Ok(json_string(v.as_ref()))
            }
        }
    }
}

fn parse_int(s: &str) -> Option<i64> {
    let digits = s.strip_prefix(['+', '-']).unwrap_or(s);
    if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    s.parse().ok()
}

fn json_string(s: &str) -> String {
    serde_json::to_string(s).unwrap_or_else(|_| "\"\"".into())
}

#[derive(Debug, Clone, Copy)]
pub struct Profile {
    pub name: &'static str,
    pub summary: &'static str,
    pub set: &'static [(&'static str, &'static str)],
}

pub static PROFILES: &[Profile] = &[
    Profile {
        name: "personal",
        summary: "Mirror everything you own. New repositories start syncing on their own.",
        set: &[
            ("filter.new_repos", "\"sync\""),
            ("filter.scope", "\"all\""),
            ("filter.skip_forks", "false"),
            ("sync.visibility", "true"),
            ("sync.on_delete", "\"delete\""),
        ],
    },
    Profile {
        name: "team",
        summary: "For when others can add you to projects: new repositories wait for your OK, repos where you are only a collaborator are left alone.",
        set: &[
            ("filter.new_repos", "\"review\""),
            ("filter.scope", "\"admin\""),
            ("filter.skip_forks", "true"),
            ("sync.visibility", "true"),
            ("sync.on_delete", "\"archive\""),
        ],
    },
    Profile {
        name: "careful",
        summary: "Only what you pick. Nothing new is synced, the GitHub copy is never made public by itself and removals are archived.",
        set: &[
            ("filter.new_repos", "\"ignore\""),
            ("filter.scope", "\"all\""),
            ("filter.skip_forks", "true"),
            ("sync.visibility", "false"),
            ("sync.on_delete", "\"archive\""),
        ],
    },
];

pub fn find_profile(name: &str) -> Option<&'static Profile> {
    let name = name.to_lowercase();
    PROFILES.iter().find(|p| p.name == name)
}

impl Profile {
    pub fn describe(&self) -> String {
        format!("{:<9} {}", self.name, self.summary)
    }
}
