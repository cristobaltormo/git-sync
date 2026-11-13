use super::ui::{ago, unix_now};
use super::{flags, load};
use crate::engine::read_state;
use crate::error::Error;

pub(super) fn cmd_status(cfg_path: &str, args: &[String]) -> Result<i32, Error> {
    let (path, _) = flags("status", cfg_path, args, |_| {})?;
    let cfg = load(&path)?;
    let Ok(st) = read_state(&cfg.paths.state_dir) else {
        println!("no state yet in {}", cfg.paths.state_dir);
        return Ok(0);
    };
    let mut rows: Vec<(String, &crate::engine::Entry)> = st
        .repos
        .values()
        .map(|x| (crate::engine::full_name(&x.owner, &x.name), x))
        .collect();
    let width = rows.iter().map(|(f, _)| f.len()).fold(4, usize::max);
    rows.sort_by(|a, b| a.0.cmp(&b.0));
    let now = unix_now();
    for (full, x) in &rows {
        let vis = match &x.gh {
            Some(g) if g.private => "private",
            Some(_) => "public",
            None => "-",
        };
        let dst = if x.gh_owner.is_empty() {
            "-".to_string()
        } else {
            format!("{}/{}", x.gh_owner, x.gh_name)
        };
        let when = if x.last_ok > 0 {
            ago(now - x.last_ok)
        } else {
            "never".into()
        };
        let mut line = format!("{full:<width$}  ->  {dst:<28} {vis:<8} {when}");
        if x.excluded {
            line.push_str("  (excluded by filter)");
        }
        if !x.managed && x.pending != 0 {
            line.push_str("  (waiting for your decision: gitsync repos)");
        }
        if !x.last_error.is_empty() {
            line.push_str("  ERROR: ");
            line.push_str(&x.last_error);
        }
        println!("{line}");
    }
    println!("{} repo(s)", rows.len());
    Ok(0)
}
