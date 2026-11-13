use std::collections::{BTreeSet, HashSet};

use super::open_app;
use super::ui::{DIM, Table, clip, heading, paint};
use crate::error::Error;
use crate::github::Target;

pub(super) fn cmd_report(cfg_path: &str, args: &[String]) -> Result<i32, Error> {
    let (a, _) = open_app(cfg_path, args, |_| {})?;
    let managed: HashSet<String> = a
        .eng
        .entries()
        .into_values()
        .filter(|x| x.managed && !x.gh_owner.is_empty())
        .map(|x| format!("{}/{}", x.gh_owner, x.gh_name).to_lowercase())
        .collect();
    let names: BTreeSet<&str> = a.cfg.accounts.values().map(String::as_str).collect();

    let mut only = Vec::new();
    for o in &names {
        for r in a.gh.list_repos(o)? {
            if !managed.contains(&format!("{o}/{}", r.name).to_lowercase()) {
                only.push(r);
            }
        }
    }
    let mut pulls = Vec::new();
    for o in &names {
        for x in a.gh.open_pulls(o)? {
            if managed.contains(&format!("{}/{}", x.owner, x.repo).to_lowercase()) {
                pulls.push(x);
            }
        }
    }

    heading("Pull requests open on the mirrors");
    if pulls.is_empty() {
        println!("{}", paint(" none", &[DIM]));
    } else {
        let mut t = Table::new(&["PULL REQUEST", "AUTHOR", "TITLE"]);
        for p in &pulls {
            t.add(vec![p.key(), p.author.clone(), clip(&p.title, 60)], &[]);
        }
        t.print();
        println!(
            "{}",
            paint(
                " gitsync never merges or closes these. A merge on GitHub would be overwritten by the next sync:\n apply the change on the source server instead.",
                &[DIM]
            )
        );
    }
    println!();
    heading("Repositories that exist only on GitHub");
    if only.is_empty() {
        println!("{}", paint(" none", &[DIM]));
    } else {
        let mut t = Table::new(&["REPOSITORY", "VISIBILITY", "LAST PUSH"]);
        for r in &only {
            let vis = if r.private { "private" } else { "public" };
            let pushed = r.pushed_at.split('T').next().unwrap_or_default();
            t.add(
                vec![r.full_name.clone(), vis.to_string(), pushed.to_string()],
                &[],
            );
        }
        t.print();
        println!(
            "{}",
            paint(
                " gitsync leaves these alone. To bring one under it, create it on the source with the same name and set sync.adopt_existing.",
                &[DIM]
            )
        );
    }
    Ok(0)
}
