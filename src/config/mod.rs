mod edit;
mod keys;
mod owner;
mod repos;

use std::collections::BTreeMap;
use std::path::Path;
use std::time::UNIX_EPOCH;

use serde::Deserialize;

use crate::config_err;
use crate::error::{Error, io_message};
use crate::logx;

pub use edit::set_values;
pub use keys::{Key, Kind, PROFILES, Profile, find_key, find_profile, keys};
pub use owner::{like_dir, tree_like_dir};
pub use repos::{RepoRule, Repos, repos_path};

#[derive(Clone, Deserialize)]
#[serde(default)]
pub struct Source {
    #[serde(rename = "type")]
    pub kind: String,
    pub url: String,
    pub token: String,
    pub token_file: String,
    pub user: String,
}

impl Default for Source {
    fn default() -> Source {
        Source {
            kind: "forgejo".into(),
            url: String::new(),
            token: String::new(),
            token_file: String::new(),
            user: String::new(),
        }
    }
}

#[derive(Clone, Deserialize)]
#[serde(default)]
pub struct GitHub {
    pub token: String,
    pub token_file: String,
    pub api_url: String,
    pub git_url: String,
}

impl Default for GitHub {
    fn default() -> GitHub {
        GitHub {
            token: String::new(),
            token_file: String::new(),
            api_url: "https://api.github.com".into(),
            git_url: "https://github.com".into(),
        }
    }
}

#[derive(Clone, Deserialize)]
#[serde(default)]
pub struct Listen {
    pub host: String,
    pub port: i64,
    pub secret: String,
    pub secret_file: String,
    pub public_url: String,
}

impl Default for Listen {
    fn default() -> Listen {
        Listen {
            host: "127.0.0.1".into(),
            port: 9001,
            secret: String::new(),
            secret_file: String::new(),
            public_url: String::new(),
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct SyncSettings {
    pub poll_interval: i64,
    pub verify_interval: i64,
    pub workers: i64,
    pub visibility: bool,
    pub metadata: bool,
    pub default_branch: bool,
    pub archived: bool,
    pub tags: bool,
    pub on_delete: String,
    pub delete_grace: i64,
    pub delete_limit: i64,
    pub adopt_existing: bool,
    pub dry_run: bool,
    pub git_timeout: i64,
}

impl Default for SyncSettings {
    fn default() -> SyncSettings {
        SyncSettings {
            poll_interval: 5,
            verify_interval: 3600,
            workers: 2,
            visibility: true,
            metadata: true,
            default_branch: true,
            archived: true,
            tags: true,
            on_delete: "delete".into(),
            delete_grace: 10,
            delete_limit: 5,
            adopt_existing: false,
            dry_run: false,
            git_timeout: 1800,
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct Filter {
    pub include: Vec<String>,
    pub exclude: Vec<String>,
    pub topic: String,
    pub skip_forks: bool,
    pub skip_mirrors: bool,
    pub skip_archived: bool,
    pub skip_private: bool,
    pub scope: String,
    pub new_repos: String,
}

impl Default for Filter {
    fn default() -> Filter {
        Filter {
            include: Vec::new(),
            exclude: Vec::new(),
            topic: String::new(),
            skip_forks: false,
            skip_mirrors: true,
            skip_archived: false,
            skip_private: false,
            scope: "all".into(),
            new_repos: "sync".into(),
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct Hooks {
    pub mode: String,
}

impl Default for Hooks {
    fn default() -> Hooks {
        Hooks {
            mode: "auto".into(),
        }
    }
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct Paths {
    pub state_dir: String,
    pub repos_file: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct Notify {
    pub url: String,
    pub format: String,
    pub events: Vec<String>,
}

impl Default for Notify {
    fn default() -> Notify {
        Notify {
            url: String::new(),
            format: "json".into(),
            events: vec!["pending".into(), "failing".into(), "pull_request".into()],
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct PullRequests {
    pub mode: String,
    pub message: String,
}

impl Default for PullRequests {
    fn default() -> PullRequests {
        PullRequests {
            mode: "leave".into(),
            message: String::new(),
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct Log {
    pub level: String,
}

impl Default for Log {
    fn default() -> Log {
        Log {
            level: "info".into(),
        }
    }
}

#[derive(Clone, Default, Deserialize)]
#[serde(default)]
pub struct Config {
    pub source: Source,
    pub github: GitHub,
    pub accounts: BTreeMap<String, String>,
    pub listen: Listen,
    pub sync: SyncSettings,
    pub filter: Filter,
    pub hooks: Hooks,
    pub paths: Paths,
    pub log: Log,
    pub notify: Notify,
    #[serde(rename = "pull_requests")]
    pub pulls: PullRequests,
    #[serde(skip)]
    pub path: String,
    #[serde(skip)]
    pub repos: Repos,
}

fn masked(s: &str) -> &'static str {
    if s.is_empty() { "" } else { "[hidden]" }
}

impl std::fmt::Debug for Source {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Source")
            .field("kind", &self.kind)
            .field("url", &self.url)
            .field("token", &masked(&self.token))
            .field("token_file", &self.token_file)
            .field("user", &self.user)
            .finish()
    }
}

impl std::fmt::Debug for GitHub {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GitHub")
            .field("token", &masked(&self.token))
            .field("token_file", &self.token_file)
            .field("api_url", &self.api_url)
            .field("git_url", &self.git_url)
            .finish()
    }
}

impl std::fmt::Debug for Listen {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Listen")
            .field("host", &self.host)
            .field("port", &self.port)
            .field("secret", &masked(&self.secret))
            .field("secret_file", &self.secret_file)
            .field("public_url", &self.public_url)
            .finish()
    }
}

pub const SOURCE_TYPES: &[&str] = &[
    "forgejo",
    "gitea",
    "gogs",
    "codeberg",
    "gitlab",
    "gitbucket",
    "onedev",
    "bitbucket-cloud",
    "bitbucket-server",
];

pub fn default_url(source_type: &str) -> &'static str {
    match source_type {
        "codeberg" => "https://codeberg.org",
        "gitlab" => "https://gitlab.com",
        "bitbucket-cloud" => "https://bitbucket.org",
        _ => "",
    }
}

pub fn default_state_dir() -> String {
    if let Some(d) = env_nonempty("STATE_DIRECTORY") {
        return d.split(':').next().unwrap_or_default().to_string();
    }
    if cfg!(windows)
        && let Some(d) = env_nonempty("LOCALAPPDATA")
    {
        return join(&d, "gitsync");
    }
    if is_root() {
        return "/var/lib/gitsync".into();
    }
    let base = env_nonempty("XDG_STATE_HOME")
        .unwrap_or_else(|| join(&join(&home_dir(), ".local"), "state"));
    join(&base, "gitsync")
}

#[cfg(unix)]
fn is_root() -> bool {
    unsafe { libc::geteuid() == 0 }
}

#[cfg(not(unix))]
fn is_root() -> bool {
    false
}

fn home_dir() -> String {
    let var = if cfg!(windows) { "USERPROFILE" } else { "HOME" };
    env_nonempty(var).unwrap_or_default()
}

pub(crate) fn env_nonempty(name: &str) -> Option<String> {
    std::env::var(name).ok().filter(|v| !v.is_empty())
}

pub(crate) fn join(dir: &str, name: &str) -> String {
    Path::new(dir).join(name).to_string_lossy().into_owned()
}

pub(crate) fn parent_dir(path: &str) -> String {
    match Path::new(path).parent() {
        Some(p) if !p.as_os_str().is_empty() => p.to_string_lossy().into_owned(),
        _ => ".".into(),
    }
}

fn file_exists(p: &str) -> bool {
    std::fs::metadata(p).is_ok_and(|m| !m.is_dir())
}

pub fn find(explicit: &str) -> String {
    let env = std::env::var("GITSYNC_CONFIG").unwrap_or_default();
    let system = system_config();
    for c in [explicit, env.as_str(), "config.toml", system.as_str()] {
        if !c.is_empty() && file_exists(c) {
            return c.to_string();
        }
    }
    if !explicit.is_empty() {
        return explicit.to_string();
    }
    if !env.is_empty() {
        return env;
    }
    system
}

pub fn system_config() -> String {
    if cfg!(windows) {
        let base = std::env::var("ProgramData").unwrap_or_default();
        return join(&join(&base, "gitsync"), "config.toml");
    }
    "/etc/gitsync/config.toml".into()
}

pub(crate) fn quote_key(k: &str) -> String {
    if !k.is_empty()
        && k.bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
    {
        return k.to_string();
    }
    quote_string(k)
}

pub(crate) fn quote_string(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 || c as u32 == 0x7f => {
                out.push_str(&format!("\\u{:04x}", c as u32));
            }
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

pub(crate) fn parse_toml(path: &str, text: &str) -> Result<toml::Table, Error> {
    text.parse::<toml::Table>().map_err(|e| {
        let mut msg = e
            .message()
            .lines()
            .map(str::trim)
            .filter(|l| !l.is_empty())
            .collect::<Vec<_>>()
            .join("; ");
        if msg.contains("escape") || msg.contains("hexadecimal") {
            msg.push_str(" (write Windows paths between single quotes: 'C:\\path\\file')");
        }
        match e.span() {
            Some(span) => {
                let (line, column, key) = locate(text, span.start);
                let key = key
                    .map(|k| format!(" (last key {k:?})"))
                    .unwrap_or_default();
                config_err!("{path}: line {line}, column {column}{key}: {msg}")
            }
            None => config_err!("{path}: {msg}"),
        }
    })
}

fn locate(text: &str, at: usize) -> (usize, usize, Option<String>) {
    let before = &text.as_bytes()[..at.min(text.len())];
    let line_start = before
        .iter()
        .rposition(|&b| b == b'\n')
        .map_or(0, |i| i + 1);
    let line = before.iter().filter(|&&b| b == b'\n').count() + 1;
    let column = before[line_start..]
        .iter()
        .filter(|&&b| b & 0xc0 != 0x80)
        .count()
        + 1;
    let lines: Vec<&str> = text.split('\n').take(line).collect();
    let mut key = None;
    let mut section = None;
    for l in lines.iter().rev() {
        let t = l.trim();
        if let Some(rest) = t.strip_prefix('[') {
            let name = rest.trim_start_matches('[');
            let name = name[..name.find(']').unwrap_or(name.len())].trim();
            section = Some(name.trim_matches('"').to_string());
            break;
        }
        if key.is_none()
            && let Some((k, _)) = t.split_once('=')
        {
            let k = k.trim().trim_matches('"');
            if keys::is_field_name(k) || repos::RULE_FIELDS.contains(&k) {
                key = Some(k.to_string());
            }
        }
    }
    let key = match (section, key) {
        (Some(s), Some(k)) if keys::is_field_name(&s) => Some(format!("{s}.{k}")),
        (_, k) => k,
    };
    (line, column, key)
}

pub(crate) fn fold_keys(table: &mut toml::Table, canonical: impl Fn(&str) -> Option<&'static str>) {
    let renames: Vec<(String, &'static str)> = table
        .keys()
        .filter_map(|k| canonical(k).filter(|c| c != k).map(|c| (k.clone(), c)))
        .collect();
    for (from, to) in renames {
        if table.contains_key(to) {
            continue;
        }
        if let Some(v) = table.remove(&from) {
            table.insert(to.to_string(), v);
        }
    }
}

pub(crate) fn unknown_keys(prefix: &str, table: &toml::Table, out: &mut Vec<String>) {
    for (k, v) in table {
        let name = if prefix.is_empty() {
            quote_key(k)
        } else {
            format!("{prefix}.{}", quote_key(k))
        };
        if let toml::Value::Table(t) = v {
            unknown_keys(&name, t, out);
        }
        out.push(name);
    }
}

pub(crate) fn report_unknown(path: &str, mut names: Vec<String>) -> Result<(), Error> {
    if names.is_empty() {
        return Ok(());
    }
    names.sort();
    Err(config_err!(
        "unknown option(s) in {path}: {}",
        names.join(", ")
    ))
}

pub(crate) fn read_text(path: &str) -> Result<Option<String>, Error> {
    match std::fs::read(path) {
        Ok(b) => String::from_utf8(b)
            .map(Some)
            .map_err(|_| config_err!("{path}: the file is not valid UTF-8")),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(config_err!("{path}: {}", io_message(&e))),
    }
}

impl Config {
    pub fn load(path: &str) -> Result<Config, Error> {
        Config::load_with(path, &env_nonempty)
    }

    pub(crate) fn load_with(
        path: &str,
        env: &dyn Fn(&str) -> Option<String>,
    ) -> Result<Config, Error> {
        let Some(text) = read_text(path)? else {
            return Err(config_err!(
                "config file not found: {path} (run `gitsync init`)"
            ));
        };
        let mut table = parse_toml(path, &text)?;
        fold_keys(&mut table, keys::canonical_section);
        for (sec, v) in table.iter_mut() {
            if let toml::Value::Table(t) = v
                && sec != "accounts"
            {
                fold_keys(t, |f| keys::canonical_field(sec, f));
            }
        }
        let mut unknown = Vec::new();
        for (sec, v) in &table {
            if sec == "accounts" {
                continue;
            }
            match (keys::is_section(sec), v) {
                (true, toml::Value::Table(t)) => {
                    for (k, sub) in t {
                        if keys::is_known(sec, k) {
                            continue;
                        }
                        let name = format!("{}.{}", quote_key(sec), quote_key(k));
                        if let toml::Value::Table(inner) = sub {
                            unknown_keys(&name, inner, &mut unknown);
                        }
                        unknown.push(name);
                    }
                }
                (true, _) => {}
                (false, v) => {
                    let name = quote_key(sec);
                    if let toml::Value::Table(inner) = v {
                        unknown_keys(&name, inner, &mut unknown);
                    }
                    unknown.push(name);
                }
            }
        }
        report_unknown(path, unknown)?;
        let mut c: Config = toml::Value::Table(table)
            .try_into()
            .map_err(|e: toml::de::Error| config_err!("{path}: {}", e.to_string().trim_end()))?;
        c.path = path.to_string();
        c.finish_with(env)?;
        c.repos = Repos::load(&repos_path(&c))?;
        Ok(c)
    }

    pub fn stamp(&self) -> String {
        if self.path.is_empty() {
            return String::new();
        }
        format!("{}|{}", stamp(&self.path), stamp(&repos_path(self)))
    }

    pub fn finish(&mut self) -> Result<(), Error> {
        self.finish_with(&env_nonempty)
    }

    fn finish_with(&mut self, env: &dyn Fn(&str) -> Option<String>) -> Result<(), Error> {
        for (name, dst) in [
            ("GITSYNC_SOURCE_TOKEN", &mut self.source.token),
            ("GITSYNC_GITHUB_TOKEN", &mut self.github.token),
            ("GITSYNC_WEBHOOK_SECRET", &mut self.listen.secret),
        ] {
            if let Some(v) = env(name)
                .map(|v| v.trim().to_string())
                .filter(|v| !v.is_empty())
            {
                *dst = v;
            }
        }
        for (val, file) in [
            (&mut self.source.token, &self.source.token_file),
            (&mut self.github.token, &self.github.token_file),
            (&mut self.listen.secret, &self.listen.secret_file),
        ] {
            if val.is_empty() && !file.is_empty() {
                *val = read_secret_file(file)?;
            }
        }
        if self.source.url.is_empty() {
            self.source.url = default_url(&self.source.kind).to_string();
        }
        self.validate()?;
        logx::hide(&self.source.token);
        logx::hide(&self.github.token);
        logx::hide(&self.listen.secret);
        logx::set_level(&self.log.level);
        Ok(())
    }

    fn validate(&mut self) -> Result<(), Error> {
        if !SOURCE_TYPES.contains(&self.source.kind.as_str()) {
            return Err(config_err!(
                "source.type must be one of: {}",
                SOURCE_TYPES.join(", ")
            ));
        }
        if !is_http_url(&self.source.url) {
            return Err(config_err!(
                "source.url must be set, e.g. \"https://git.example.com\""
            ));
        }
        if self.source.token.is_empty() {
            return Err(config_err!(
                "source.token is missing (or use token_file / GITSYNC_SOURCE_TOKEN)"
            ));
        }
        if self.github.token.is_empty() {
            return Err(config_err!(
                "github.token is missing (or use token_file / GITSYNC_GITHUB_TOKEN)"
            ));
        }
        if self.accounts.is_empty() {
            return Err(config_err!(
                "[accounts] is empty: map at least one source owner to a GitHub owner"
            ));
        }
        for (k, v) in &self.accounts {
            if v.is_empty() {
                return Err(config_err!(
                    "accounts.{k} must be a GitHub user or organization"
                ));
            }
        }
        for (name, v) in [
            ("source.token", &self.source.token),
            ("github.token", &self.github.token),
            ("listen.secret", &self.listen.secret),
        ] {
            if v.chars().any(char::is_control) {
                return Err(config_err!("{name} contains control characters"));
            }
        }
        if self.listen.secret.is_empty() {
            return Err(config_err!(
                "listen.secret is missing: webhooks are only accepted when signed (`gitsync init` generates one)"
            ));
        }
        if !(1..=65535).contains(&self.listen.port) {
            return Err(config_err!("listen.port must be a port number"));
        }
        if !one_of(&self.sync.on_delete, &["delete", "archive", "ignore"]) {
            return Err(config_err!(
                "sync.on_delete must be delete, archive or ignore"
            ));
        }
        if !one_of(&self.filter.scope, &["all", "admin"]) {
            return Err(config_err!("filter.scope must be all or admin"));
        }
        if !one_of(&self.filter.new_repos, &["sync", "review", "ignore"]) {
            return Err(config_err!(
                "filter.new_repos must be sync, review or ignore"
            ));
        }
        if !self.notify.url.is_empty() && !is_http_url(&self.notify.url) {
            return Err(config_err!("notify.url must be an http(s) URL"));
        }
        if !one_of(&self.notify.format, &["json", "text", "slack", "discord"]) {
            return Err(config_err!(
                "notify.format must be json, text, slack or discord"
            ));
        }
        for ev in &self.notify.events {
            if !one_of(ev, &["pending", "failing", "pull_request"]) {
                return Err(config_err!(
                    "notify.events: unknown event {ev:?} (pending, failing, pull_request)"
                ));
            }
        }
        if !one_of(&self.pulls.mode, &["leave", "comment"]) {
            return Err(config_err!("pull_requests.mode must be leave or comment"));
        }
        if !one_of(&self.hooks.mode, &["auto", "system", "repo", "none"]) {
            return Err(config_err!("hooks.mode must be auto, system, repo or none"));
        }
        for (name, v) in [
            ("poll_interval", self.sync.poll_interval),
            ("verify_interval", self.sync.verify_interval),
            ("delete_grace", self.sync.delete_grace),
            ("delete_limit", self.sync.delete_limit),
            ("git_timeout", self.sync.git_timeout),
        ] {
            if v < 0 {
                return Err(config_err!("sync.{name} must be 0 or more"));
            }
        }
        if self.sync.workers < 1 {
            return Err(config_err!("sync.workers must be at least 1"));
        }
        if self.paths.state_dir.is_empty() {
            self.paths.state_dir = default_state_dir();
        }
        Ok(())
    }

    pub fn source_url(&self) -> &str {
        self.source.url.trim_end_matches('/')
    }

    pub fn hook_url(&self) -> String {
        if !self.listen.public_url.is_empty() {
            return self.listen.public_url.clone();
        }
        let host = match self.listen.host.as_str() {
            "" | "0.0.0.0" | "::" => "127.0.0.1",
            h => h,
        };
        format!("http://{}/hook", host_port(host, self.listen.port))
    }

    pub fn github_owner(&self, source_owner: &str) -> Option<&str> {
        self.accounts
            .iter()
            .find(|(k, _)| eq_fold(k, source_owner))
            .map(|(_, v)| v.as_str())
    }

    pub fn source_scope(&self) -> String {
        let url = self.source_url();
        match url.find("://") {
            Some(i) => format!("{}{}/", url[..i].to_ascii_lowercase(), &url[i..]),
            None => format!("{url}/"),
        }
    }
}

fn eq_fold(a: &str, b: &str) -> bool {
    if a.is_ascii() && b.is_ascii() {
        return a.eq_ignore_ascii_case(b);
    }
    a.chars()
        .flat_map(char::to_lowercase)
        .eq(b.chars().flat_map(char::to_lowercase))
}

pub fn host_port(host: &str, port: i64) -> String {
    if host.contains(':') {
        format!("[{host}]:{port}")
    } else {
        format!("{host}:{port}")
    }
}

pub fn is_http_url(s: &str) -> bool {
    let rest = s
        .strip_prefix("https://")
        .or_else(|| s.strip_prefix("http://"));
    rest.and_then(|r| r.chars().next())
        .is_some_and(|c| c != '/' && !c.is_whitespace())
}

pub(crate) fn one_of(v: &str, opts: &[&str]) -> bool {
    opts.contains(&v)
}

fn read_secret_file(p: &str) -> Result<String, Error> {
    std::fs::read(p)
        .map(|b| String::from_utf8_lossy(&b).trim().to_string())
        .map_err(|e| config_err!("cannot read {p}: {}", io_message(&e)))
}

fn stamp(p: &str) -> String {
    let Ok(st) = std::fs::metadata(p) else {
        return "-".into();
    };
    let nanos = st
        .modified()
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map(|d| d.as_nanos() as i64)
        .unwrap_or(0);
    format!("{}:{}", base36(nanos), base36(st.len() as i64))
}

fn base36(n: i64) -> String {
    if n == 0 {
        return "0".into();
    }
    let neg = n < 0;
    let mut v = n.unsigned_abs();
    let mut digits = Vec::new();
    while v > 0 {
        digits.push(b"0123456789abcdefghijklmnopqrstuvwxyz"[(v % 36) as usize]);
        v /= 36;
    }
    if neg {
        digits.push(b'-');
    }
    digits.reverse();
    String::from_utf8(digits).unwrap_or_default()
}

#[cfg(test)]
mod tests;
