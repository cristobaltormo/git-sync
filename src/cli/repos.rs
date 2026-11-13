use std::collections::BTreeMap;

use serde::Serialize;

use super::inventory::{
    Counts, Item, ST_ERROR, ST_FILTERED, ST_IGNORED, ST_QUEUED, ST_SYNCED, ST_WAITING, inventory,
    resolve,
};
use super::ui::{BOLD, DIM, GREEN, RED, Style, Table, YELLOW, ago, clip, pad, paint, unix_now};
use super::{open_app, tui};
use crate::config::{Config, RepoRule, Repos, repos_path};
use crate::error::Error;

const REPOS_USAGE: &str = "Usage: gitsync repos [command]

With no command in a terminal, opens the interactive selector.

  list [--status S] [--json]     show every repository and what gitsync does with it
  allow <repo>... | --pending    sync these repositories
  ignore <repo>...               never sync these repositories
  reset <repo>...                forget the decision, the general rules apply again
  set <repo> key=value ...       per-repository options: name, keep_private, tags,
                                 metadata, prune, on_delete, note (key= clears one)

A repo is owner/name, just the name, or a pattern such as 'my-org/*'.
";

pub(super) fn cmd_repos(cfg_path: &str, args: &[String]) -> Result<i32, Error> {
    let (sub, rest) = match args.first() {
        Some(a) if !a.starts_with('-') => (a.as_str(), &args[1..]),
        _ => ("", args),
    };
    if sub == "help" {
        print!("{REPOS_USAGE}");
        return Ok(0);
    }
    if sub.is_empty() && super::ui::stdin_is_terminal() && super::ui::stdout_is_terminal() {
        return tui::cmd_repos_tui(cfg_path, rest);
    }
    match sub {
        "" | "list" | "ls" => repos_list(cfg_path, rest),
        "allow" | "ignore" | "reset" => repos_decide(cfg_path, sub, rest),
        "set" => repos_set(cfg_path, rest),
        _ => Err(Error::Config(format!(
            "unknown command {sub:?}\n\n{REPOS_USAGE}"
        ))),
    }
}

pub(crate) fn status_style(st: &str) -> Style {
    match st {
        ST_SYNCED => GREEN,
        ST_ERROR => RED,
        ST_WAITING | ST_QUEUED => YELLOW,
        _ => DIM,
    }
}

pub(crate) fn note(it: &Item) -> String {
    let mut parts: Vec<String> = Vec::new();
    match it.status {
        ST_WAITING => parts.push(format!("new: gitsync repos allow {}", it.full())),
        ST_IGNORED | ST_FILTERED => {
            parts.push(it.verdict.why.clone());
            if it.managed() {
                parts.push("GitHub copy left as it is".into());
            }
        }
        ST_ERROR => parts.push(clip(&it.entry.last_error, 60)),
        ST_QUEUED => parts.push("will be created on GitHub".into()),
        _ => {}
    }
    if let Some(r) = &it.verdict.rule {
        if !r.name.is_empty() {
            parts.push(format!("as {}", r.name));
        }
        if r.keep_private == Some(true) {
            parts.push("kept private".into());
        }
    }
    parts.join("; ")
}

fn last_sync(it: &Item) -> String {
    if it.has && it.entry.last_ok > 0 {
        return ago(unix_now() - it.entry.last_ok);
    }
    "-".into()
}

#[derive(Serialize)]
struct ListRow<'a> {
    repo: String,
    status: &'a str,
    #[serde(skip_serializing_if = "String::is_empty")]
    github: String,
    #[serde(skip_serializing_if = "str::is_empty")]
    reason: &'a str,
    #[serde(skip_serializing_if = "str::is_empty")]
    role: &'a str,
    private: bool,
    #[serde(skip_serializing_if = "str::is_empty")]
    error: &'a str,
}

fn repos_list(cfg_path: &str, args: &[String]) -> Result<i32, Error> {
    let (mut status, mut json) = (None, None);
    let (a, fs) = open_app(cfg_path, args, |fs| {
        status = Some(fs.string(
            &["status"],
            "",
            "only show: synced, waiting, ignored, filtered, error, queued",
        ));
        json = Some(fs.bool("json", "machine-readable output"));
    })?;
    let status = status.map_or("", |s| fs.get(s));
    let as_json = json.is_some_and(|j| fs.on(j));
    let items = inventory(&a)?;
    let shown: Vec<&Item> = items
        .iter()
        .filter(|it| status.is_empty() || it.status == status)
        .collect();
    if as_json {
        let out: Vec<ListRow> = shown
            .iter()
            .map(|it| ListRow {
                repo: it.full(),
                status: it.status,
                github: it.target(),
                reason: &it.verdict.why,
                role: it.repo.role,
                private: it.repo.private,
                error: &it.entry.last_error,
            })
            .collect();
        let mut text = serde_json::to_string_pretty(&out)?;
        text.push('\n');
        print!("{text}");
        return Ok(0);
    }
    let c = Counts::of(&items);
    println!(
        "{}  {} -> GitHub",
        paint("gitsync", &[BOLD]),
        a.cfg.source_url()
    );
    print!(
        " {} repositories: {} synced, {} waiting for you, {} ignored, {} filtered out",
        items.len(),
        c.get(ST_SYNCED) + c.get(ST_ERROR) + c.get(ST_QUEUED),
        c.get(ST_WAITING),
        c.get(ST_IGNORED),
        c.get(ST_FILTERED)
    );
    if c.get(ST_ERROR) > 0 {
        print!(
            ", {}",
            paint(&format!("{} with errors", c.get(ST_ERROR)), &[RED])
        );
    }
    print!("\n\n");
    let mut t = Table::new(&["STATUS", "REPOSITORY", "GITHUB", "LAST SYNC", "NOTES"]);
    for it in &shown {
        let mut dst = it.target();
        if dst.is_empty() {
            dst = "-".into();
        }
        t.add(
            vec![
                it.status.to_string(),
                it.full(),
                dst,
                last_sync(it),
                note(it),
            ],
            &[status_style(it.status)],
        );
    }
    t.print();
    let waiting = c.get(ST_WAITING);
    if waiting > 0 && status.is_empty() {
        let noun = if waiting == 1 { "y is" } else { "ies are" };
        println!(
            "\n{}",
            paint(
                &format!(
                    " {waiting} new repositor{noun} waiting. Sync them with `gitsync repos allow <repo>` or `--pending`, skip them with `gitsync repos ignore <repo>`."
                ),
                &[YELLOW]
            )
        );
    }
    Ok(0)
}

pub(crate) fn save_repos(
    cfg: &Config,
    change: impl FnOnce(&mut Repos) -> Result<(), Error>,
) -> Result<(), Error> {
    let path = if cfg.repos.path.is_empty() {
        repos_path(cfg)
    } else {
        cfg.repos.path.clone()
    };
    let mut r = Repos::load(&path)?;
    change(&mut r)?;
    if let Err(e) = r.save() {
        if e.to_string().contains("permission denied") {
            return Err(Error::Config(format!(
                "cannot write {}: permission denied (run as the user that owns the config, e.g. with sudo)",
                r.path
            )));
        }
        return Err(e);
    }
    Ok(())
}

fn applied(cfg: &Config) {
    println!("Saved to {}.", repos_path(cfg));
    if cfg.sync.poll_interval == 0 {
        println!("A running gitsync reloads on `systemctl reload gitsync` (polling is off).");
        return;
    }
    println!(
        "A running gitsync picks it up within {} seconds.",
        cfg.sync.poll_interval.max(1)
    );
}

fn repos_decide(cfg_path: &str, verb: &str, args: &[String]) -> Result<i32, Error> {
    let mut pending = None;
    let (a, fs) = open_app(cfg_path, args, |fs| {
        pending = Some(fs.bool("pending", "every repository that is waiting for a decision"));
    })?;
    let pending = pending.is_some_and(|p| fs.on(p));
    let items = inventory(&a)?;
    let mut chosen: Vec<usize> = Vec::new();
    if pending {
        chosen.extend((0..items.len()).filter(|&i| items[i].status == ST_WAITING));
    }
    if fs.nargs() > 0 {
        chosen.extend(resolve(&items, fs.args())?);
    }
    if chosen.is_empty() {
        if pending {
            println!("Nothing is waiting for a decision.");
            return Ok(0);
        }
        return Err(Error::Config(
            "name at least one repository (see `gitsync repos help`)".into(),
        ));
    }
    let value = match verb {
        "allow" => Some(true),
        "ignore" => Some(false),
        _ => None,
    };
    save_repos(&a.cfg, |r| {
        for &i in &chosen {
            r.edit(&items[i].full(), |x| x.sync = value)?;
        }
        Ok(())
    })?;
    let past = match verb {
        "allow" => "will be synced",
        "ignore" => "will be ignored",
        _ => "follows the general rules again",
    };
    for &i in &chosen {
        println!("  {} {past}", pad(&items[i].full(), 40));
    }
    applied(&a.cfg);
    if verb == "ignore" {
        for &i in &chosen {
            if items[i].managed() {
                println!(
                    "Note: {} keeps its GitHub copy; gitsync stops updating it and never deletes it for this.",
                    items[i].full()
                );
            }
        }
    }
    Ok(0)
}

fn parse_bool_opt(v: &str) -> Result<Option<bool>, Error> {
    match v.to_lowercase().as_str() {
        "" | "default" => Ok(None),
        "true" | "yes" | "on" | "1" => Ok(Some(true)),
        "false" | "no" | "off" | "0" => Ok(Some(false)),
        _ => Err(Error::Config(format!("{v:?} is not true or false"))),
    }
}

fn apply_option(x: &mut RepoRule, key: &str, val: &str) -> Result<(), Error> {
    match key {
        "name" => x.name = val.to_string(),
        "on_delete" => x.on_delete = val.to_string(),
        "note" => x.note = val.to_string(),
        "keep_private" => x.keep_private = parse_bool_opt(val)?,
        "tags" => x.tags = parse_bool_opt(val)?,
        "metadata" => x.metadata = parse_bool_opt(val)?,
        "prune" => x.prune = parse_bool_opt(val)?,
        _ => {
            return Err(Error::Config(format!(
                "unknown option {key:?} (name, keep_private, tags, metadata, prune, on_delete, note)"
            )));
        }
    }
    Ok(())
}

fn repos_set(cfg_path: &str, args: &[String]) -> Result<i32, Error> {
    let (a, fs) = open_app(cfg_path, args, |_| {})?;
    if fs.nargs() < 2 {
        return Err(Error::Config(
            "usage: gitsync repos set <repo> key=value ...".into(),
        ));
    }
    let items = inventory(&a)?;
    let chosen = resolve(&items, &fs.args()[..1])?;
    if chosen.len() != 1 {
        return Err(Error::Config(format!(
            "{:?} matches {} repositories; set options on one at a time",
            fs.arg(0),
            chosen.len()
        )));
    }
    let mut opts = BTreeMap::new();
    for kv in &fs.args()[1..] {
        let Some((k, v)) = kv.split_once('=') else {
            return Err(Error::Config(format!("{kv:?} is not key=value")));
        };
        opts.insert(k.trim().to_string(), v.trim().to_string());
    }
    let it = &items[chosen[0]];
    let full = it.full();
    save_repos(&a.cfg, |r| {
        let mut opt_err = None;
        r.edit(&full, |x| {
            for (k, v) in &opts {
                if let Err(e) = apply_option(x, k, v)
                    && opt_err.is_none()
                {
                    opt_err = Some(e);
                }
            }
        })?;
        opt_err.map_or(Ok(()), Err)
    })?;
    println!("{full} updated.");
    applied(&a.cfg);
    Ok(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn options_parse_like_go() {
        let mut r = RepoRule::default();
        apply_option(&mut r, "keep_private", "yes").unwrap();
        apply_option(&mut r, "tags", "off").unwrap();
        apply_option(&mut r, "name", "x").unwrap();
        assert_eq!(r.keep_private, Some(true));
        assert_eq!(r.tags, Some(false));
        assert_eq!(r.name, "x");
        apply_option(&mut r, "tags", "default").unwrap();
        assert_eq!(r.tags, None);
        assert_eq!(
            apply_option(&mut r, "prune", "maybe")
                .unwrap_err()
                .to_string(),
            "\"maybe\" is not true or false"
        );
        assert!(apply_option(&mut r, "colour", "x").is_err());
    }
}
