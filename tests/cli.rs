use std::process::{Command, Output};

fn gitsync(args: &[&str]) -> (i32, String, String) {
    let Output {
        status,
        stdout,
        stderr,
    } = Command::new(env!("CARGO_BIN_EXE_gitsync"))
        .args(args)
        .env_remove("GITSYNC_CONFIG")
        .env("NO_COLOR", "1")
        .output()
        .unwrap();
    (
        status.code().unwrap_or(-1),
        String::from_utf8_lossy(&stdout).into_owned(),
        String::from_utf8_lossy(&stderr).into_owned(),
    )
}

#[test]
fn version() {
    let (code, out, _) = gitsync(&["version"]);
    assert_eq!(code, 0);
    assert!(out.starts_with("gitsync "), "{out}");
    let (code, again, _) = gitsync(&["--version"]);
    assert_eq!((code, again), (0, out));
}

#[test]
fn unknown_command() {
    let (code, _, err) = gitsync(&["frobnicate"]);
    assert_eq!(code, 2);
    assert!(err.contains("unknown command \"frobnicate\""), "{err}");
}

#[test]
fn no_command_shows_usage() {
    let (code, _, err) = gitsync(&[]);
    assert_eq!(code, 2);
    assert!(err.contains("Usage: gitsync"), "{err}");
}

#[test]
fn help_goes_to_stdout() {
    let (code, out, _) = gitsync(&["help"]);
    assert_eq!(code, 0);
    assert!(out.contains("Usage: gitsync [-c config.toml] <command>"));
}

#[test]
fn missing_config_points_to_init() {
    let (code, _, err) = gitsync(&["-c", "/nonexistent/config.toml", "run"]);
    assert_eq!(code, 2);
    assert!(err.contains("gitsync init"), "{err}");
}

#[test]
fn bad_flag_prints_the_flag_usage() {
    let (code, _, err) = gitsync(&["run", "--nope"]);
    assert_eq!(code, 2);
    assert!(
        err.starts_with("flag provided but not defined: -nope\nUsage of run:\n"),
        "{err}"
    );
    assert!(err.contains("  -dry-run\n    \tlog what would happen, change nothing\n"));
    assert!(
        err.ends_with("error: flag provided but not defined: -nope\n"),
        "{err}"
    );
}

#[test]
fn status_without_state() {
    let dir = std::env::temp_dir().join(format!("gitsync-cli-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let cfg = dir.join("config.toml");
    let state = dir.join("state");
    std::fs::write(
        &cfg,
        format!(
            "[source]\ntype = \"forgejo\"\nurl = \"https://git.example.com\"\ntoken = \"abcdefgh\"\n\n[github]\ntoken = \"ijklmnop\"\n\n[accounts]\nalice = \"alice-gh\"\n\n[listen]\nsecret = \"s3cret-value\"\n\n[paths]\nstate_dir = {:?}\n",
            state.to_string_lossy()
        ),
    )
    .unwrap();
    let c = cfg.to_string_lossy().into_owned();
    let (code, out, _) = gitsync(&["-c", &c, "status"]);
    assert_eq!(code, 0);
    assert!(out.starts_with("no state yet in "), "{out}");
    std::fs::create_dir_all(&state).unwrap();
    std::fs::write(
        state.join("state.json"),
        r#"{"version":1,"repos":{"7":{"owner":"alice","name":"api","gh_owner":"alice-gh","gh_name":"api","managed":true,"last_error":"boom"}}}"#,
    )
    .unwrap();
    let (code, out, _) = gitsync(&["-c", &c, "status"]);
    assert_eq!(code, 0, "{out}");
    assert!(out.contains("alice/api  ->  alice-gh/api"), "{out}");
    assert!(out.contains("never  ERROR: boom"), "{out}");
    assert!(out.ends_with("1 repo(s)\n"));
    let (code, out, _) = gitsync(&["-c", &c, "config", "get", "sync.poll_interval"]);
    assert_eq!((code, out.as_str()), (0, "5\n"));
    let (code, _, err) = gitsync(&["-c", &c, "config", "get", "poll"]);
    assert_eq!(code, 2);
    assert!(err.contains("did you mean sync.poll_interval"), "{err}");
    let (code, out, _) = gitsync(&["-c", &c, "config", "show", "--all"]);
    assert_eq!(code, 0);
    assert!(out.contains("[accounts]\n  alice = alice-gh\n"), "{out}");
    let _ = std::fs::remove_dir_all(&dir);
}
