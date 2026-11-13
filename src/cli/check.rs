use std::collections::HashSet;
use std::process::Command;

use super::{build, flags, load};
use crate::error::Error;
use crate::github;

fn mark(state: &str, msg: impl std::fmt::Display) {
    println!("  {state:<4} {msg}");
}

pub(super) fn cmd_check(cfg_path: &str, args: &[String]) -> Result<i32, Error> {
    let (path, _) = flags("check", cfg_path, args, |_| {})?;
    let cfg = load(&path)?;
    println!("config {}: ok", cfg.path);
    let a = build(cfg)?;
    let cfg = &a.cfg;
    let gh = github::Client::new(cfg, false);
    let mut problems = 0;

    let me = match a.src.whoami() {
        Ok(me) => me,
        Err(e) => {
            mark("FAIL", format_args!("source: {e}"));
            return Ok(1);
        }
    };
    let admin = if me.admin { " (admin)" } else { "" };
    mark(
        "ok",
        format_args!("source token works, logged in as {}{admin}", me.login),
    );
    if !me.system_hooks && cfg.hooks.mode == "auto" && a.src.hooks().is_some() {
        mark("info", "webhooks will be created per repo");
    }

    let (login, scopes) = match gh.whoami() {
        Ok(x) => x,
        Err(e) => {
            mark("FAIL", format_args!("GitHub: {e}"));
            return Ok(1);
        }
    };
    mark(
        "ok",
        format_args!("GitHub token works, logged in as {login}"),
    );
    match scopes {
        Some(scopes) => {
            let have: HashSet<&str> = scopes.split(',').map(str::trim).collect();
            if !have.contains("repo") {
                mark("FAIL", "GitHub token has no 'repo' scope");
                problems += 1;
            }
            if cfg.sync.on_delete == "delete" && !have.contains("delete_repo") {
                mark(
                    "warn",
                    "no 'delete_repo' scope: deleting mirrors will fail (or set on_delete = \"archive\")",
                );
            }
        }
        None => mark(
            "info",
            "fine-grained token: it needs Administration and Contents write on the target repos",
        ),
    }

    for (src, dst) in &cfg.accounts {
        match a.src.list(src) {
            Err(e) => {
                mark("FAIL", format_args!("source account '{src}': {e}"));
                problems += 1;
            }
            Ok(repos) => mark(
                "ok",
                format_args!("source account '{src}': {} repo(s) visible", repos.len()),
            ),
        }
        match gh.owner_type(dst) {
            Err(e) => {
                mark("FAIL", format_args!("GitHub account '{dst}': {e}"));
                problems += 1;
            }
            Ok(kind) if kind == "User" && !eq_fold(dst, &login) => {
                mark(
                    "FAIL",
                    format_args!(
                        "GitHub account '{dst}' is another user; the token can only create repos under '{login}' or an organization"
                    ),
                );
                problems += 1;
            }
            Ok(kind) => mark(
                "ok",
                format_args!("GitHub account '{dst}' ({})", kind.to_lowercase()),
            ),
        }
    }

    match Command::new("git").arg("--version").output() {
        Ok(out) if out.status.success() => {
            mark("ok", String::from_utf8_lossy(&out.stdout).trim());
        }
        _ => {
            mark("FAIL", "git is not installed");
            problems += 1;
        }
    }
    let h = cfg.listen.host.as_str();
    if h != "127.0.0.1" && h != "localhost" && h != "::1" && cfg.listen.public_url.is_empty() {
        mark(
            "warn",
            "listening beyond localhost without listen.public_url: the webhooks will point at 127.0.0.1",
        );
    }
    mark("info", format_args!("webhook URL: {}", cfg.hook_url()));
    if problems == 0 {
        println!("all good");
        return Ok(0);
    }
    println!("{problems} problem(s)");
    Ok(1)
}

pub(super) fn eq_fold(a: &str, b: &str) -> bool {
    a.chars()
        .flat_map(char::to_lowercase)
        .eq(b.chars().flat_map(char::to_lowercase))
}
