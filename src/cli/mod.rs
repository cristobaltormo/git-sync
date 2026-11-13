mod check;
mod configcmd;
mod flags;
mod hooks;
mod init;
mod install;
mod inventory;
mod plan;
mod report;
mod repos;
mod run;
mod signals;
mod status;
mod sync;
mod tui;
mod tui_model;
mod tui_view;
mod ui;
mod why;

use std::sync::Arc;

use crate::config::{self, Config};
use crate::engine::Engine;
use crate::error::Error;
use crate::forge::{self, Provider};
use crate::github;
use crate::logx;
use crate::mirror;

use flags::FlagSet;

const USAGE: &str = "gitsync {v}: mirror repos from a self-hosted git server to GitHub, live.

Usage: gitsync [-c config.toml] <command>

  init       write a config file, asking a few questions
  check      test the config, tokens and permissions
  run        run the daemon (--dry-run to only log what would happen)
  sync       mirror everything once and exit (owner/name ... for just some)
  status     show what has been synced
  repos      choose which repositories are synced (interactive)
  why        explain why a repository is or is not synced
  plan       show what the next sync would do, changing nothing
  report     pull requests on the mirrors and repos that exist only on GitHub
  config     show or change settings, apply a profile
  hooks      create the webhooks on the git server (--force, --remove)
  install    install as a service: systemd on Linux, a scheduled task on Windows
  uninstall  remove what install set up
  version    print the version

Without -c, the config is looked for in $GITSYNC_CONFIG, ./config.toml and
/etc/gitsync/config.toml (%ProgramData%\\gitsync\\config.toml on Windows).
";

type Command = fn(&str, &[String]) -> Result<i32, Error>;

fn usage(version: &str) -> String {
    USAGE.replace("{v}", version)
}

fn command(name: &str) -> Option<Command> {
    Some(match name {
        "run" => run::cmd_run,
        "sync" => sync::cmd_sync,
        "check" => check::cmd_check,
        "status" => status::cmd_status,
        "hooks" => hooks::cmd_hooks,
        "repos" => repos::cmd_repos,
        "config" => configcmd::cmd_config,
        "why" => why::cmd_why,
        "plan" => plan::cmd_plan,
        "report" => report::cmd_report,
        "init" => init::cmd_init,
        "install" => install::cmd_install,
        "uninstall" => install::cmd_uninstall,
        _ => return None,
    })
}

pub fn run(version: &str, args: &[String]) -> i32 {
    let mut global = FlagSet::new("gitsync");
    let cfg_flag = global.string(&["c", "config"], "", "config file");
    let mut start = 0;
    while let Some(a) = args.get(start) {
        match a.as_str() {
            "-v" | "-version" | "--version" => {
                println!("gitsync {version}");
                return 0;
            }
            "-h" | "-help" | "--help" => {
                print!("{}", usage(version));
                return 0;
            }
            "-c" | "-config" | "--config" => start += 2,
            s if s.starts_with('-') && s.len() > 1 && s != "--" => start += 1,
            _ => break,
        }
    }
    match global.try_parse(args) {
        Ok(()) => {}
        Err(flags::Failure::Help) => {
            eprint!("{}", usage(version));
            return 2;
        }
        Err(flags::Failure::Bad(msg)) => {
            eprintln!("{msg}");
            eprint!("{}", usage(version));
            return 2;
        }
    }
    let rest = global.args();
    let Some(name) = rest.first() else {
        eprint!("{}", usage(version));
        return 2;
    };
    match name.as_str() {
        "version" | "-v" | "--version" => {
            println!("gitsync {version}");
            return 0;
        }
        "help" | "-h" | "--help" => {
            print!("{}", usage(version));
            return 0;
        }
        _ => {}
    }
    let Some(cmd) = command(name) else {
        eprintln!("unknown command {name:?}\n");
        eprint!("{}", usage(version));
        return 2;
    };
    if name != "run" && name != "sync" {
        signals::default_sigpipe();
    }
    let cfg_path = global.get(cfg_flag).to_string();
    match cmd(&cfg_path, &rest[1..]) {
        Ok(code) => code,
        Err(e) => {
            eprintln!("error: {}", logx::clean(&e.to_string()));
            if e.is_config() { 2 } else { 1 }
        }
    }
}

fn flags(
    name: &'static str,
    cfg_path: &str,
    args: &[String],
    setup: impl FnOnce(&mut FlagSet),
) -> Result<(String, FlagSet), Error> {
    let mut fs = FlagSet::new(name);
    let c = fs.string(&["c", "config"], cfg_path, "config file");
    setup(&mut fs);
    fs.parse(args)?;
    Ok((fs.get(c).to_string(), fs))
}

pub(crate) struct App {
    pub(crate) cfg: Arc<Config>,
    pub(crate) src: Arc<dyn Provider>,
    pub(crate) gh: Arc<github::Client>,
    pub(crate) eng: Arc<Engine>,
}

fn load(cfg_path: &str) -> Result<Config, Error> {
    Config::load(&config::find(cfg_path))
}

fn build(cfg: Config) -> Result<App, Error> {
    let src = forge::new(&cfg).map_err(|e| Error::Config(e.to_string()))?;
    let dry = cfg.sync.dry_run;
    let gh = Arc::new(github::Client::new(&cfg, dry));
    let mir = Arc::new(mirror::Git::new(&cfg, src.clone(), dry));
    let cfg = Arc::new(cfg);
    let eng = Engine::new(cfg.clone(), src.clone(), gh.clone(), mir)?;
    Ok(App { cfg, src, gh, eng })
}

fn open_app(
    cfg_path: &str,
    args: &[String],
    setup: impl FnOnce(&mut FlagSet),
) -> Result<(App, FlagSet), Error> {
    let (path, fs) = flags("repos", cfg_path, args, setup)?;
    let app = build(load(&path)?)?;
    Ok((app, fs))
}

fn create_state_dir(cfg: &Config) -> Result<(), Error> {
    std::fs::create_dir_all(&cfg.paths.state_dir).map_err(|e| {
        Error::Config(format!(
            "cannot create {}: {}",
            cfg.paths.state_dir,
            crate::error::io_message(&e)
        ))
    })
}

#[cfg(test)]
mod tests;
