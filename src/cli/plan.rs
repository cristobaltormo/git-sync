use std::collections::{HashMap, HashSet};

use serde::Serialize;

use super::inventory::{Item, ST_FILTERED, ST_IGNORED, ST_WAITING, inventory};
use super::ui::{DIM, GREEN, RED, Style, Table, YELLOW, clip, paint};
use super::{App, open_app};
use crate::error::Error;
use crate::github::Target;

#[derive(Serialize)]
struct PlanRow {
    repo: String,
    action: &'static str,
    #[serde(skip_serializing_if = "String::is_empty")]
    detail: String,
}

const ORDER: [&str; 8] = [
    "blocked",
    "create",
    "rename",
    "update",
    "removed",
    "waiting",
    "up to date",
    "skip",
];

const TALLY: [&str; 8] = [
    "create",
    "update",
    "rename",
    "blocked",
    "removed",
    "waiting",
    "up to date",
    "skip",
];

pub(super) fn cmd_plan(cfg_path: &str, args: &[String]) -> Result<i32, Error> {
    let mut json = None;
    let (a, fs) = open_app(cfg_path, args, |fs| {
        json = Some(fs.bool("json", "machine-readable output"));
    })?;
    let as_json = json.is_some_and(|j| fs.on(j));
    let items = inventory(&a)?;
    let mut rows = Vec::with_capacity(items.len());
    let mut seen = HashSet::new();
    for it in &items {
        seen.insert(it.repo.id);
        rows.push(plan_item(&a, it)?);
    }
    let mut removed: Vec<_> = a
        .eng
        .entries()
        .into_iter()
        .filter(|(id, x)| !seen.contains(id) && x.managed)
        .map(|(_, x)| x)
        .collect();
    removed.sort_by(|x, y| (&x.owner, &x.name).cmp(&(&y.owner, &y.name)));
    for x in removed {
        let full = crate::engine::full_name(&x.owner, &x.name);
        let mut od = a.cfg.sync.on_delete.as_str();
        if let Some(r) = a.cfg.repos.rule(&full)
            && !r.on_delete.is_empty()
        {
            od = &r.on_delete;
        }
        let act = match od {
            "delete" => "delete the GitHub copy",
            "archive" => "archive the GitHub copy",
            "ignore" => "leave the GitHub copy",
            _ => "",
        };
        rows.push(PlanRow {
            repo: full,
            action: "removed",
            detail: format!("gone from the source: {act}"),
        });
    }
    rows.sort_by_key(|r| order(r.action));
    if as_json {
        let mut out = serde_json::to_string_pretty(&rows)?;
        out.push('\n');
        print!("{out}");
        return Ok(0);
    }
    let mut tally: HashMap<&str, usize> = HashMap::new();
    let mut t = Table::new(&["ACTION", "REPOSITORY", "DETAIL"]);
    for r in &rows {
        *tally.entry(r.action).or_default() += 1;
        if r.action == "up to date" || r.action == "skip" {
            continue;
        }
        t.add(
            vec![r.action.to_string(), r.repo.clone(), r.detail.clone()],
            &[plan_style(r.action)],
        );
    }
    if !t.rows.is_empty() {
        t.print();
        println!();
    }
    let parts: Vec<String> = TALLY
        .iter()
        .filter_map(|k| tally.get(k).map(|n| format!("{n} {k}")))
        .collect();
    println!(" {}", parts.join(", "));
    println!(
        "{}",
        paint(
            " Nothing was changed. `gitsync run` or `gitsync sync` applies it.",
            &[DIM]
        )
    );
    Ok(0)
}

fn order(a: &str) -> usize {
    ORDER.iter().position(|k| *k == a).unwrap_or(99)
}

fn plan_style(a: &str) -> Style {
    match a {
        "create" | "update" | "rename" => GREEN,
        "blocked" | "removed" => RED,
        "waiting" => YELLOW,
        _ => DIM,
    }
}

fn row(it: &Item, action: &'static str, detail: String) -> PlanRow {
    PlanRow {
        repo: it.full(),
        action,
        detail,
    }
}

fn plan_item(a: &App, it: &Item) -> Result<PlanRow, Error> {
    let r = &it.repo;
    match it.status {
        ST_WAITING => {
            return Ok(row(
                it,
                "waiting",
                "new repository, needs your decision".into(),
            ));
        }
        ST_IGNORED | ST_FILTERED => {
            let mut detail = it.verdict.why.clone();
            if it.managed() {
                detail.push_str(" (GitHub copy stays)");
            }
            return Ok(row(it, "skip", detail));
        }
        _ => {}
    }
    let Some(owner) = a.cfg.github_owner(&r.owner) else {
        return Ok(row(
            it,
            "skip",
            format!("no GitHub account mapped for {}", r.owner),
        ));
    };
    let name = match &it.verdict.rule {
        Some(x) if !x.name.is_empty() => x.name.as_str(),
        _ => r.name.as_str(),
    };
    let target = format!("{owner}/{name}");
    if !it.managed() {
        let g =
            a.gh.get(owner, name)
                .map_err(|e| Error::Config(format!("asking GitHub about {target}: {e}")))?;
        return Ok(match g {
            None if r.private || !a.cfg.sync.visibility => {
                row(it, "create", format!("new private repository {target}"))
            }
            None => row(
                it,
                "create",
                format!("new private repository {target}, then make it public if the source is"),
            ),
            Some(_) if a.cfg.sync.adopt_existing => row(
                it,
                "update",
                format!("{target} already exists: adopted, overwritten to match"),
            ),
            Some(_) => row(
                it,
                "blocked",
                format!(
                    "{target} already exists on GitHub and was not created by gitsync (sync.adopt_existing)"
                ),
            ),
        });
    }
    let x = &it.entry;
    Ok(
        if x.gh_name != name || !super::check::eq_fold(&x.gh_owner, owner) {
            row(
                it,
                "rename",
                format!("{}/{} -> {target}", x.gh_owner, x.gh_name),
            )
        } else if !x.last_error.is_empty() {
            row(
                it,
                "update",
                format!("retry after: {}", clip(&x.last_error, 70)),
            )
        } else if x.sig != r.sig() || x.updated != r.updated {
            row(it, "update", "changed on the source".into())
        } else {
            row(it, "up to date", String::new())
        },
    )
}
