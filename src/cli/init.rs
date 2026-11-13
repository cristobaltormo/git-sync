use std::collections::BTreeMap;
use std::io::{BufRead, Write};

use super::flags;
use super::ui::stdin_is_terminal;
use crate::config::{self, Config, PROFILES, Profile, SOURCE_TYPES, find_profile, set_values};
use crate::error::Error;
use crate::{forge, github, logx};

fn read_line() -> String {
    let mut line = String::new();
    let _ = std::io::stdin().lock().read_line(&mut line);
    line
}

fn ask(prompt: &str, def: &str, secret: bool) -> String {
    let label = if def.is_empty() {
        format!("{prompt}: ")
    } else {
        format!("{prompt} [{def}]: ")
    };
    let mut err = std::io::stderr();
    let _ = err.write_all(label.as_bytes());
    let _ = err.flush();
    let hide = secret && stdin_is_terminal();
    let line = if hide {
        let guard = echo::off();
        let line = read_line();
        drop(guard);
        let _ = writeln!(err);
        line
    } else {
        read_line()
    };
    let line = line.trim();
    if line.is_empty() {
        def.to_string()
    } else {
        line.to_string()
    }
}

fn yes(prompt: &str, def: bool) -> bool {
    let hint = if def { " (Y/n)" } else { " (y/N)" };
    let v = ask(&format!("{prompt}{hint}"), "", false).to_lowercase();
    if v.is_empty() {
        return def;
    }
    v.starts_with('y')
}

fn quote(s: &str) -> String {
    serde_json::to_string(s).unwrap_or_else(|_| "\"\"".into())
}

#[cfg(unix)]
mod echo {
    pub(super) struct Guard(Option<libc::termios>);

    pub(super) fn off() -> Guard {
        unsafe {
            let mut t: libc::termios = std::mem::zeroed();
            if libc::tcgetattr(0, &mut t) != 0 {
                return Guard(None);
            }
            let old = t;
            t.c_lflag &= !libc::ECHO;
            if libc::tcsetattr(0, libc::TCSANOW, &t) != 0 {
                return Guard(None);
            }
            Guard(Some(old))
        }
    }

    impl Drop for Guard {
        fn drop(&mut self) {
            if let Some(old) = self.0 {
                unsafe {
                    libc::tcsetattr(0, libc::TCSANOW, &old);
                }
            }
        }
    }
}

#[cfg(windows)]
mod echo {
    use windows_sys::Win32::System::Console::{
        CONSOLE_MODE, ENABLE_ECHO_INPUT, GetConsoleMode, GetStdHandle, STD_INPUT_HANDLE,
        SetConsoleMode,
    };

    pub(super) struct Guard(Option<CONSOLE_MODE>);

    pub(super) fn off() -> Guard {
        unsafe {
            let h = GetStdHandle(STD_INPUT_HANDLE);
            let mut mode: CONSOLE_MODE = 0;
            if GetConsoleMode(h, &mut mode) == 0 {
                return Guard(None);
            }
            if SetConsoleMode(h, mode & !ENABLE_ECHO_INPUT) == 0 {
                return Guard(None);
            }
            Guard(Some(mode))
        }
    }

    impl Drop for Guard {
        fn drop(&mut self) {
            if let Some(mode) = self.0 {
                unsafe {
                    SetConsoleMode(GetStdHandle(STD_INPUT_HANDLE), mode);
                }
            }
        }
    }
}

pub(crate) fn choose_profile(flag: &str) -> Result<&'static Profile, Error> {
    if !flag.is_empty() {
        return find_profile(flag).ok_or_else(|| {
            Error::Config(format!(
                "no profile called {flag:?} (personal, team, careful)"
            ))
        });
    }
    println!("\nHow should repositories that appear later be handled?");
    for (i, p) in PROFILES.iter().enumerate() {
        println!("  {}) {}", i + 1, p.describe());
    }
    loop {
        let v = ask("Choose one", "personal", false).to_lowercase();
        if let Ok(n) = v.parse::<usize>()
            && (1..=PROFILES.len()).contains(&n)
        {
            return Ok(&PROFILES[n - 1]);
        }
        if let Some(p) = find_profile(&v) {
            return Ok(p);
        }
        println!("Type a number or a name from the list.");
    }
}

fn random_hex(n: usize) -> Result<String, Error> {
    use ring::rand::SecureRandom;
    let mut buf = vec![0u8; n];
    ring::rand::SystemRandom::new()
        .fill(&mut buf)
        .map_err(|_| crate::err!("no random source"))?;
    Ok(forge::hex(&buf))
}

pub(super) fn cmd_init(cfg_path: &str, args: &[String]) -> Result<i32, Error> {
    let (mut force, mut profile) = (None, None);
    let (path, fs) = flags("init", cfg_path, args, |fs| {
        force = Some(fs.bool("force", "overwrite an existing config"));
        profile = Some(fs.string(
            &["profile"],
            "",
            "personal, team or careful: skip the question about new repositories",
        ));
    })?;
    let force = force.is_some_and(|f| fs.on(f));
    let profile = profile.map_or("", |p| fs.get(p));
    let path = if path.is_empty() {
        "config.toml".to_string()
    } else {
        path
    };
    if std::fs::metadata(&path).is_ok() && !force {
        return Err(Error::Config(format!(
            "{path} already exists (use --force to overwrite)"
        )));
    }
    println!(
        "This asks a few questions and writes {path}. Nothing is changed on either side yet.\n"
    );

    let typ = ask(
        &format!("Source server type ({})", SOURCE_TYPES.join(", ")),
        "forgejo",
        false,
    )
    .to_lowercase();
    if !SOURCE_TYPES.contains(&typ.as_str()) {
        return Err(Error::Config(format!(
            "type must be one of: {}",
            SOURCE_TYPES.join(", ")
        )));
    }
    let mut tmp = Config::default();
    tmp.source.kind = typ.clone();
    let url = ask(
        "Source URL as this machine reaches it (e.g. https://git.example.com)",
        config::default_url(&typ),
        false,
    )
    .trim_end_matches('/')
    .to_string();
    println!(
        "Create an access token on the source server with read access to the repositories,\nand write access to their webhooks (admin if you want one system-wide webhook)."
    );
    let src_token = ask("Source token", "", true);
    let port: i64 = ask("Port for the webhook listener", "9001", false)
        .parse()
        .map_err(|_| Error::Config("that is not a port number".into()))?;
    let secret = random_hex(24)?;

    tmp.source.url = url.clone();
    tmp.source.token = src_token.clone();
    logx::hide(&src_token);
    let src = forge::new(&tmp).map_err(|e| Error::Config(e.to_string()))?;
    let me = src
        .whoami()
        .map_err(|e| Error::Config(format!("could not log in to the source: {e}")))?;
    println!("Logged in as {}.\n", me.login);

    println!(
        "Create a GitHub token (Settings, Developer settings, Personal access tokens, classic)\nwith the scopes 'repo' and 'delete_repo'."
    );
    let gh_token = ask("GitHub token", "", true);
    logx::hide(&gh_token);
    tmp.github.token = gh_token.clone();
    let (gh_login, _) = github::Client::new(&tmp, false)
        .whoami()
        .map_err(|e| Error::Config(format!("could not log in to GitHub: {e}")))?;
    println!("Logged in to GitHub as {gh_login}.\n");

    let mut lines = Vec::new();
    for o in ask("Which owners to mirror (comma separated)", &me.login, false).split(',') {
        let o = o.trim();
        if o.is_empty() {
            continue;
        }
        let def = if super::check::eq_fold(o, &me.login) {
            gh_login.as_str()
        } else {
            o
        };
        let dst = ask(&format!("GitHub owner for {o}"), def, false);
        lines.push(format!("{} = {}", quote(o), quote(&dst)));
    }
    let vis = yes(
        "Should public repos on the source become public on GitHub (private ones stay private)?",
        true,
    );
    let on_delete = if yes(
        "When a repo is deleted on the source, delete it on GitHub too? (no = archive it)",
        true,
    ) {
        "delete"
    } else {
        "archive"
    };
    let chosen = choose_profile(profile)?;
    let text = render_template(
        &typ,
        &url,
        &src_token,
        &gh_token,
        &lines.join("\n"),
        port,
        &secret,
        vis,
        on_delete,
    );
    write_private(&path, &text)?;
    let set: BTreeMap<String, String> = chosen
        .set
        .iter()
        .filter(|(k, _)| k.starts_with("filter."))
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect();
    set_values(&path, &set)?;
    println!(
        "\nWrote {path} (only you can read it), profile {}.",
        chosen.name
    );
    println!("Next: `gitsync check`, then `gitsync run --dry-run` to see what it would do,");
    if chosen.name != "personal" {
        println!(
            "then `gitsync repos` to pick the repositories to sync (nothing is synced until you do),"
        );
    }
    println!("then `gitsync run`, or `sudo gitsync install` to keep it running.");
    println!(
        "Change anything later with `gitsync repos`, `gitsync config` or by editing the file."
    );
    Ok(0)
}

#[allow(clippy::too_many_arguments)]
fn render_template(
    typ: &str,
    url: &str,
    src_token: &str,
    gh_token: &str,
    accounts: &str,
    port: i64,
    secret: &str,
    visibility: bool,
    on_delete: &str,
) -> String {
    format!(
        "# gitsync configuration. Every option is explained in examples/config.example.toml.

[source]
type = {typ}
url = {url}
token = {src_token}

[github]
token = {gh_token}

# source owner = GitHub owner
[accounts]
{accounts}

[listen]
host = \"127.0.0.1\"
port = {port}
secret = {secret}

[sync]
visibility = {visibility}
on_delete = {on_delete}
",
        typ = quote(typ),
        url = quote(url),
        src_token = quote(src_token),
        gh_token = quote(gh_token),
        secret = quote(secret),
        on_delete = quote(on_delete),
    )
}

fn write_private(path: &str, text: &str) -> Result<(), Error> {
    let mut opts = std::fs::OpenOptions::new();
    opts.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600);
    }
    let mut f = opts
        .open(path)
        .map_err(|e| crate::err!("open {path}: {}", crate::error::io_message(&e)))?;
    f.write_all(text.as_bytes())?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn choose_profile_flag() {
        assert_eq!(choose_profile("team").unwrap().name, "team");
        assert!(choose_profile("nope").is_err());
    }

    #[test]
    fn template_loads_as_a_config() {
        let dir = crate::testutil::TempDir::new();
        let path = dir.path().join("config.toml");
        let path = path.to_string_lossy().into_owned();
        let text = render_template(
            "forgejo",
            "https://git.example.com",
            "src-token-123",
            "gh-token-456",
            "\"alice\" = \"alice-gh\"",
            9001,
            "abcdef",
            true,
            "delete",
        );
        write_private(&path, &text).unwrap();
        let mut set = BTreeMap::new();
        set.insert("filter.new_repos".to_string(), "\"review\"".to_string());
        set_values(&path, &set).unwrap();
        let cfg = Config::load(&path).unwrap();
        assert_eq!(cfg.source.url, "https://git.example.com");
        assert_eq!(cfg.github_owner("ALICE"), Some("alice-gh"));
        assert_eq!(cfg.filter.new_repos, "review");
        assert_eq!(cfg.listen.port, 9001);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&path).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o600);
        }
    }

    #[test]
    fn placeholders_in_answers_stay_literal() {
        let text = render_template(
            "forgejo",
            "https://h/{secret}",
            "{port}",
            "t",
            "",
            1,
            "hidden",
            false,
            "ignore",
        );
        assert!(text.contains("url = \"https://h/{secret}\"\n"), "{text}");
        assert!(text.contains("token = \"{port}\"\n"), "{text}");
        assert_eq!(text.matches("hidden").count(), 1);
    }
}
