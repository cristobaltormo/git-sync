use std::collections::HashSet;
use std::time::Duration;

use super::{build, create_state_dir, flags, load};
use crate::engine::Job;
use crate::error::Error;

pub(super) fn cmd_sync(cfg_path: &str, args: &[String]) -> Result<i32, Error> {
    let mut dry = None;
    let (path, fs) = flags("sync", cfg_path, args, |fs| {
        dry = Some(fs.bool("dry-run", "log what would happen, change nothing"));
    })?;
    let mut cfg = load(&path)?;
    if dry.is_some_and(|d| fs.on(d)) {
        cfg.sync.dry_run = true;
    }
    let a = build(cfg)?;
    create_state_dir(&a.cfg)?;
    a.eng.start();
    let wanted: HashSet<String> = fs.args().iter().map(|r| r.to_lowercase()).collect();
    let repos = a.eng.list_all()?;
    let mut n = 0;
    for r in repos {
        if !wanted.is_empty()
            && !wanted.contains(&r.full().to_lowercase())
            && !wanted.contains(&r.name.to_lowercase())
        {
            continue;
        }
        if a.eng.selected(&r).0 {
            a.eng.enqueue(Job::for_repo(r, "sync", true, true));
            n += 1;
        }
    }
    if !wanted.is_empty() && n == 0 {
        return Err(Error::Config(format!(
            "no repo matched {}",
            fs.args().join(", ")
        )));
    }
    a.eng.drain(Duration::ZERO);
    if wanted.is_empty() {
        a.eng.reconcile(false, "sync");
        a.eng.drain(Duration::ZERO);
        if a.eng.has_missing() {
            let grace = a.cfg.sync.delete_grace.max(0) as u64 + 1;
            std::thread::sleep(Duration::from_secs(grace));
            a.eng.reconcile(false, "sync");
            a.eng.drain(Duration::ZERO);
        }
    }
    a.eng.shutdown();
    let mut failed: Vec<_> = a
        .eng
        .entries()
        .into_values()
        .filter(|x| !x.last_error.is_empty())
        .collect();
    failed.sort_by(|x, y| (&x.owner, &x.name).cmp(&(&y.owner, &y.name)));
    for x in &failed {
        eprintln!(
            "failed: {}: {}",
            crate::engine::full_name(&x.owner, &x.name),
            x.last_error
        );
    }
    println!("{n} repo(s) processed, {} failed", failed.len());
    Ok(if failed.is_empty() { 0 } else { 1 })
}
