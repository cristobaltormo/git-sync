use super::inventory::{
    Item, ST_ERROR, ST_FILTERED, ST_IGNORED, ST_QUEUED, ST_SYNCED, ST_WAITING, inventory, resolve,
};
use super::repos::status_style;
use super::ui::{BOLD, RED, ago, date_minutes, heading, pad, paint, unix_now};
use super::{App, open_app};
use crate::config::{Config, RepoRule};
use crate::error::Error;

fn yn(b: bool) -> &'static str {
    if b { "yes" } else { "no" }
}

pub(super) fn cmd_why(cfg_path: &str, args: &[String]) -> Result<i32, Error> {
    let (a, fs) = open_app(cfg_path, args, |_| {})?;
    if fs.nargs() != 1 {
        return Err(Error::Config("usage: gitsync why <repo>".into()));
    }
    let items = inventory(&a)?;
    let found = resolve(&items, fs.args())?;
    for (n, &i) in found.iter().enumerate() {
        if n > 0 {
            println!();
        }
        explain(&a, &items[i]);
    }
    Ok(0)
}

fn label(s: &str) -> String {
    pad(s, 14)
}

fn rule_bool(r: Option<&RepoRule>, pick: impl Fn(&RepoRule) -> Option<bool>, def: bool) -> bool {
    r.and_then(pick).unwrap_or(def)
}

fn explain(a: &App, it: &Item) {
    let r = &it.repo;
    let v = &it.verdict;
    let full = it.full();
    heading(&full);
    let verdict = match it.status {
        ST_SYNCED => "synced to GitHub",
        ST_ERROR => "synced, but the last attempt failed",
        ST_QUEUED => "selected, will be created on GitHub on the next pass",
        ST_WAITING => "waiting for your decision",
        ST_IGNORED => "ignored, by your choice",
        ST_FILTERED => "not synced",
        _ => "",
    };
    println!(
        "  {} {}",
        label("Status"),
        paint(verdict, &[status_style(it.status), BOLD])
    );
    if !v.why.is_empty() {
        println!("  {} {}", label("Because"), v.why);
    } else if matches!(it.status, ST_SYNCED | ST_QUEUED | ST_ERROR) {
        println!("  {} no rule excludes it", label("Because"));
    }
    let role = if r.role.is_empty() {
        "unknown for this server"
    } else {
        r.role
    };
    println!(
        "  {} private {}, fork {}, pull mirror {}, archived {}, your role: {role}",
        label("On the source"),
        yn(r.private),
        yn(r.fork),
        yn(r.mirror),
        yn(r.archived)
    );
    if !r.topics.is_empty() {
        println!("  {} [{}]", label("Topics"), r.topics.join(" "));
    }
    let cfg: &Config = &a.cfg;
    let rule = v.rule.as_ref();
    let name = match rule {
        Some(x) if !x.name.is_empty() => x.name.as_str(),
        _ => r.name.as_str(),
    };
    if let Some(dst) = cfg.github_owner(&r.owner) {
        println!("  {} {dst}/{name}", label("Goes to"));
    }
    let mut on_delete = cfg.sync.on_delete.as_str();
    if let Some(x) = rule
        && !x.on_delete.is_empty()
    {
        on_delete = &x.on_delete;
    }
    let opts = [
        format!("tags {}", yn(rule_bool(rule, |x| x.tags, cfg.sync.tags))),
        format!(
            "description and topics {}",
            yn(rule_bool(rule, |x| x.metadata, cfg.sync.metadata))
        ),
        format!(
            "follows visibility {}",
            yn(cfg.sync.visibility && !rule_bool(rule, |x| x.keep_private, false))
        ),
        format!(
            "branches only on GitHub: {}",
            if rule_bool(rule, |x| x.prune, true) {
                "deleted"
            } else {
                "kept"
            }
        ),
        format!("if removed: {on_delete}"),
    ];
    println!("  {} {}", label("Options"), opts.join("; "));
    if it.has && it.entry.last_ok > 0 {
        println!(
            "  {} {}",
            label("Last sync"),
            ago(unix_now() - it.entry.last_ok)
        );
    }
    if !it.entry.last_error.is_empty() {
        println!(
            "  {} {}",
            label("Last error"),
            paint(&it.entry.last_error, &[RED])
        );
    }
    if it.entry.pending != 0 {
        println!(
            "  {} since {}",
            label("Waiting"),
            date_minutes(it.entry.pending)
        );
    }
    println!();
    match it.status {
        ST_WAITING => println!(
            "  To sync it:    gitsync repos allow {full}\n  To skip it:    gitsync repos ignore {full}"
        ),
        ST_IGNORED => println!(
            "  To sync it:    gitsync repos allow {full}\n  To undo this:  gitsync repos reset {full}"
        ),
        ST_FILTERED => println!("  To sync it anyway:  gitsync repos allow {full}"),
        _ => println!("  To stop syncing it:  gitsync repos ignore {full}"),
    }
}
