use std::collections::BTreeMap;
use std::path::Path;
use std::process::Command;

use super::flags;
use super::ui::{BOLD, DIM, pad, paint};
use crate::config::{self, Config, Key, PROFILES, find_key, find_profile, keys, set_values};
use crate::engine::send_test;
use crate::error::Error;

const CONFIG_USAGE: &str = "Usage: gitsync config <command>

  show [--all]                  current settings (--all includes the ones left at their default)
  get <key>                     print one setting, e.g. sync.poll_interval
  set <key> <value>...          change a setting; comments in the file are kept and the
                                result is validated before anything is written
  profile [name]                list the ready-made profiles, or apply one
  edit                          open the file in your editor and validate it afterwards
  path                          print the config file location
  test-notify                   send a test message to notify.url
";

pub(super) fn cmd_config(cfg_path: &str, args: &[String]) -> Result<i32, Error> {
    let (sub, rest) = match args.first() {
        Some(a) if !a.starts_with('-') => (a.as_str(), &args[1..]),
        _ => ("show", args),
    };
    let mut all = None;
    let (path, fs) = flags("config", cfg_path, rest, |fs| {
        all = Some(fs.bool("all", "show: include the settings left at their default"));
    })?;
    let file = config::find(&path);
    match sub {
        "help" => {
            print!("{CONFIG_USAGE}");
            return Ok(0);
        }
        "path" => {
            println!("{file}");
            return Ok(0);
        }
        "edit" => return config_edit(&file),
        _ => {}
    }
    let cfg = Config::load(&file)?;
    match sub {
        "show" | "list" => {
            let all = all.is_some_and(|a| fs.on(a))
                || fs.args().iter().any(|a| a == "--all" || a == "-all");
            config_show(&cfg, all);
            Ok(0)
        }
        "get" => {
            if fs.nargs() != 1 {
                return Err(Error::Config("usage: gitsync config get <key>".into()));
            }
            let Some(k) = find_key(fs.arg(0)) else {
                return Err(unknown_key(fs.arg(0)));
            };
            println!("{}", shown(&k, &cfg));
            Ok(0)
        }
        "set" => config_set(&file, &cfg, fs.args()),
        "profile" => config_profile(&file, fs.args()),
        "test-notify" => {
            if cfg.notify.url.is_empty() {
                return Err(Error::Config("notify.url is not set".into()));
            }
            send_test(&cfg.notify.url, &cfg.notify.format)?;
            println!("Test message sent.");
            Ok(0)
        }
        _ => Err(Error::Config(format!(
            "unknown command {sub:?}\n\n{CONFIG_USAGE}"
        ))),
    }
}

fn unknown_key(name: &str) -> Error {
    let lower = name.to_lowercase();
    let near: Vec<&str> = keys()
        .iter()
        .filter(|k| k.name.contains(&lower))
        .map(|k| k.name)
        .collect();
    if !near.is_empty() && near.len() < 8 {
        return Error::Config(format!(
            "no setting called {name:?}; did you mean {}?",
            near.join(", ")
        ));
    }
    Error::Config(format!(
        "no setting called {name:?} (see `gitsync config show --all`)"
    ))
}

fn shown(k: &Key, c: &Config) -> String {
    if k.secret {
        if k.get(c).is_empty() {
            return "(not set)".into();
        }
        return "(set, hidden)".into();
    }
    k.get(c)
}

fn config_show(cfg: &Config, all: bool) {
    let def = Config::default();
    let mut section = "";
    for k in keys() {
        let is_def = k.get(cfg) == k.get(&def);
        if !all && is_def && !k.secret {
            continue;
        }
        let (sec, name) = k.name.split_once('.').unwrap_or((k.name, ""));
        if sec != section {
            if !section.is_empty() {
                println!();
            }
            section = sec;
            println!("{}", paint(&format!("[{sec}]"), &[BOLD]));
        }
        let mut val = shown(k, cfg);
        if val.is_empty() {
            val = paint("(empty)", &[DIM]);
        }
        let mut line = format!("  {name:<18} {val}");
        if is_def && !k.secret {
            line = paint(&format!("{line}   default"), &[DIM]);
        }
        println!("{line}");
    }
    if !cfg.accounts.is_empty() {
        println!();
        println!("{}", paint("[accounts]", &[BOLD]));
        let w = cfg.accounts.keys().map(|s| s.len()).max().unwrap_or(0);
        for (s, d) in &cfg.accounts {
            println!("  {} = {d}", pad(s, w));
        }
    }
}

fn config_set(file: &str, cfg: &Config, args: &[String]) -> Result<i32, Error> {
    if args.len() < 2 {
        return Err(Error::Config(
            "usage: gitsync config set <key> <value>...".into(),
        ));
    }
    let Some(k) = find_key(&args[0]) else {
        return Err(unknown_key(&args[0]));
    };
    if k.secret {
        return Err(Error::Config(format!(
            "{} is a secret: put it in the file by hand, or use token_file / the environment, so it does not end up in your shell history",
            k.name
        )));
    }
    let lit = k.literal(&args[1..])?;
    let mut kv = BTreeMap::new();
    kv.insert(k.name.to_string(), lit.clone());
    set_values(file, &kv)?;
    println!("{} = {lit}", k.name);
    println!("{}", reload_hint(cfg));
    Ok(0)
}

pub(crate) fn reload_hint(cfg: &Config) -> String {
    if cfg.sync.poll_interval == 0 {
        return "Saved. A running gitsync applies it on `systemctl reload gitsync` (polling is off)."
            .into();
    }
    format!(
        "Saved. A running gitsync applies it within {} seconds (listen.* needs a restart).",
        cfg.sync.poll_interval.max(1)
    )
}

fn config_profile(file: &str, args: &[String]) -> Result<i32, Error> {
    let Some(name) = args.first() else {
        for p in PROFILES {
            println!(" {}", p.describe());
        }
        println!(
            "\nApply one with `gitsync config profile <name>`. It only changes the handful of settings it names."
        );
        return Ok(0);
    };
    let Some(p) = find_profile(name) else {
        return Err(Error::Config(format!(
            "no profile called {name:?} (personal, team, careful)"
        )));
    };
    let set: BTreeMap<String, String> = p
        .set
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect();
    set_values(file, &set)?;
    println!("Profile {} applied:", p.name);
    for (k, v) in &set {
        println!("  {k:<20} = {v}");
    }
    if let Ok(cfg) = Config::load(file) {
        println!("{}", reload_hint(&cfg));
    }
    Ok(0)
}

fn look_path(name: &str) -> bool {
    let Some(paths) = std::env::var_os("PATH") else {
        return false;
    };
    std::env::split_paths(&paths).any(|d| {
        let p = d.join(name);
        std::fs::metadata(&p).is_ok_and(|m| m.is_file() && executable(&m))
    })
}

#[cfg(unix)]
fn executable(m: &std::fs::Metadata) -> bool {
    use std::os::unix::fs::PermissionsExt;
    m.permissions().mode() & 0o111 != 0
}

#[cfg(not(unix))]
fn executable(_: &std::fs::Metadata) -> bool {
    true
}

fn config_edit(file: &str) -> Result<i32, Error> {
    let mut editor = std::env::var("VISUAL").unwrap_or_default();
    if editor.is_empty() {
        editor = std::env::var("EDITOR").unwrap_or_default();
    }
    if editor.is_empty() {
        if let Some(c) = ["nano", "vim", "vi"].into_iter().find(|c| look_path(c)) {
            editor = c.to_string();
        }
        if cfg!(windows) {
            editor = "notepad".into();
        }
    }
    if editor.is_empty() {
        return Err(Error::Config("no editor found: set $EDITOR".into()));
    }
    let mut parts = editor.split_whitespace();
    let prog = parts.next().unwrap_or_default();
    let status = Command::new(prog)
        .args(parts)
        .arg(Path::new(file))
        .status()
        .map_err(|e| crate::err!("exec: {prog:?}: {}", crate::error::io_message(&e)))?;
    if !status.success() {
        return Err(crate::err!("{status}"));
    }
    if let Err(e) = Config::load(file) {
        return Err(Error::Config(format!(
            "saved, but the file has a problem: {e}"
        )));
    }
    println!("The file is valid.");
    Ok(0)
}
