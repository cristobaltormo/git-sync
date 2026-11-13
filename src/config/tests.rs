use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use super::*;
use crate::testutil::TempDir;

const MINIMAL: &str = r#"
[source]
url = "https://git.example.com"
token = "src-token-123456"
[github]
token = "gh-token-123456"
[accounts]
alice = "alice-gh"
[listen]
secret = "s3cret-value-123"
"#;

fn write(dir: &TempDir, body: &str) -> String {
    let p = dir.path().join("config.toml");
    std::fs::write(&p, body).unwrap();
    set_private(&p);
    p.to_string_lossy().into_owned()
}

#[cfg(unix)]
fn set_private(p: &Path) {
    owner::set_mode(p, 0o600).unwrap();
}

#[cfg(not(unix))]
fn set_private(_: &Path) {}

fn no_env(_: &str) -> Option<String> {
    None
}

fn load(path: &str) -> Result<Config, Error> {
    Config::load_with(path, &no_env)
}

#[test]
fn defaults() {
    let d = TempDir::new();
    let c = load(&write(&d, MINIMAL)).unwrap();
    assert_eq!(c.sync.poll_interval, 5);
    assert!(c.sync.visibility);
    assert_eq!(c.sync.on_delete, "delete");
    assert_eq!(c.hook_url(), "http://127.0.0.1:9001/hook");
    assert_eq!(
        c.github_owner("ALICE"),
        Some("alice-gh"),
        "accounts must be case-insensitive"
    );
    assert_eq!(c.github_owner("bob"), None);
}

#[test]
fn overrides_keep_other_defaults() {
    let d = TempDir::new();
    let c = load(&write(
        &d,
        &format!("{MINIMAL}\n[sync]\npoll_interval = 2\nvisibility = false\n"),
    ))
    .unwrap();
    assert_eq!(c.sync.poll_interval, 2);
    assert!(!c.sync.visibility);
    assert!(c.sync.metadata);
}

#[test]
fn errors() {
    let cases = [
        ("unknown option", format!("{MINIMAL}\n[sync]\nnope = 1\n")),
        (
            "source.token",
            MINIMAL.replacen(r#"token = "src-token-123456""#, "", 1),
        ),
        (
            "listen.secret",
            MINIMAL.replacen(r#"secret = "s3cret-value-123""#, "", 1),
        ),
        (
            "on_delete",
            format!("{MINIMAL}\n[sync]\non_delete = \"explode\"\n"),
        ),
        ("workers", format!("{MINIMAL}\n[sync]\nworkers = 0\n")),
        (
            "source.url",
            "[source]\ntype = \"forgejo\"\nurl = \"nope\"\n".to_string(),
        ),
        (
            "source.type",
            MINIMAL.replacen("[source]", "[source]\ntype = \"cvs\"", 1),
        ),
    ];
    for (want, body) in cases {
        let d = TempDir::new();
        match load(&write(&d, &body)) {
            Err(e) => {
                assert!(
                    e.to_string().contains(want),
                    "want error containing {want:?}, got {e}"
                );
                assert!(e.is_config(), "{want:?} should be a config error");
            }
            Ok(_) => panic!("want error containing {want:?}"),
        }
    }
}

#[test]
fn unknown_options_are_named() {
    let d = TempDir::new();
    let body = format!("{MINIMAL}\n[sync]\nnope = 1\n[extra]\nx = 1\n");
    let path = write(&d, &body);
    let err = load(&path).err().unwrap().to_string();
    assert_eq!(
        err,
        format!("unknown option(s) in {path}: extra, extra.x, sync.nope")
    );
}

#[test]
fn bad_escapes_suggest_single_quotes() {
    let d = TempDir::new();
    let body = MINIMAL.replacen(
        "[github]",
        "[paths]\nstate_dir = \"C:\\Users\\x\"\n[github]",
        1,
    );
    let err = load(&write(&d, &body)).err().unwrap().to_string();
    assert!(err.contains("single quotes"), "{err}");
}

#[test]
fn wrong_types_are_config_errors() {
    let d = TempDir::new();
    let err = load(&write(
        &d,
        &format!("{MINIMAL}\n[sync]\nworkers = \"two\"\n"),
    ))
    .err()
    .unwrap();
    assert!(err.is_config());
}

#[test]
fn missing_file_suggests_init() {
    let err = load("/nonexistent/x.toml").err().unwrap();
    assert!(err.to_string().contains("init"), "{err}");
    assert!(err.is_config());
}

#[test]
fn secrets_from_file_and_environment() {
    let d = TempDir::new();
    let tok = d.path().join("tok");
    std::fs::write(&tok, "from-file-token\n").unwrap();
    let body = MINIMAL.replacen(
        r#"token = "src-token-123456""#,
        &format!("token_file = '{}'", tok.display()),
        1,
    );
    let path = write(&d, &body);
    let c = load(&path).unwrap();
    assert_eq!(c.source.token, "from-file-token");
    let env = |k: &str| (k == "GITSYNC_SOURCE_TOKEN").then(|| "from-env-token".to_string());
    let c = Config::load_with(&path, &env).unwrap();
    assert_eq!(c.source.token, "from-env-token");
    let missing = MINIMAL.replacen(
        r#"token = "src-token-123456""#,
        "token_file = '/nonexistent/tok'",
        1,
    );
    let err = load(&write(&d, &missing)).err().unwrap().to_string();
    assert!(err.starts_with("cannot read /nonexistent/tok: "));
    if cfg!(unix) {
        assert_eq!(
            err,
            "cannot read /nonexistent/tok: no such file or directory"
        );
    }
}

#[test]
fn default_urls() {
    for (typ, want) in [
        ("gitlab", "https://gitlab.com"),
        ("codeberg", "https://codeberg.org"),
        ("bitbucket-cloud", "https://bitbucket.org"),
    ] {
        let d = TempDir::new();
        let body = MINIMAL
            .replacen(r#"url = "https://git.example.com""#, "", 1)
            .replacen("[source]", &format!("[source]\ntype = \"{typ}\""), 1);
        let c = load(&write(&d, &body)).unwrap();
        assert_eq!(c.source.url, want);
    }
}

#[test]
fn hook_url() {
    let mut c = Config::default();
    c.listen.host = "0.0.0.0".into();
    assert_eq!(c.hook_url(), "http://127.0.0.1:9001/hook");
    c.listen.public_url = "https://sync.example.com/hook".into();
    assert_eq!(c.hook_url(), "https://sync.example.com/hook");
    c.listen.public_url.clear();
    c.listen.host = "::1".into();
    assert_eq!(c.hook_url(), "http://[::1]:9001/hook");
}

#[test]
fn small_helpers() {
    assert!(is_http_url("https://x"));
    assert!(is_http_url("http://x/y"));
    assert!(!is_http_url("https:///x"));
    assert!(!is_http_url("ftp://x"));
    assert!(!is_http_url("nope"));
    assert_eq!(base36(0), "0");
    assert_eq!(base36(35), "z");
    assert_eq!(base36(36), "10");
    assert_eq!(base36(-37), "-11");
    let mut c = Config::default();
    c.source.url = "HTTPS://Git.Example.com/".into();
    assert_eq!(c.source_scope(), "https://Git.Example.com/");
    assert_eq!(c.source_url(), "HTTPS://Git.Example.com");
    assert_eq!(quote_key("a/b"), "\"a/b\"");
    assert_eq!(quote_key("plain_key-1"), "plain_key-1");
}

#[test]
fn stamp_changes_with_the_files() {
    let d = TempDir::new();
    let path = write(&d, MINIMAL);
    let c = load(&path).unwrap();
    let before = c.stamp();
    assert!(before.ends_with("|-"), "{before}");
    std::fs::write(d.path().join("repos.toml"), "").unwrap();
    assert_ne!(c.stamp(), before);
    assert_eq!(Config::default().stamp(), "");
}

#[test]
fn repos_file_sits_next_to_the_config() {
    let d = TempDir::new();
    let path = write(&d, MINIMAL);
    let c = load(&path).unwrap();
    assert_eq!(PathBuf::from(repos_path(&c)), d.path().join("repos.toml"));
    let mut c = Config::default();
    c.paths.state_dir = "/var/lib/gitsync".into();
    assert_eq!(
        PathBuf::from(repos_path(&c)),
        PathBuf::from("/var/lib/gitsync").join("repos.toml")
    );
}

fn repos_file(dir: &TempDir) -> String {
    dir.path().join("repos.toml").to_string_lossy().into_owned()
}

#[test]
fn repos_round_trip() {
    let d = TempDir::new();
    let p = repos_file(&d);
    let mut r = Repos::load(&p).unwrap();
    r.edit("My-Org/App", |x| {
        x.sync = Some(true);
        x.name = "app-mirror".into();
    })
    .unwrap();
    r.save().unwrap();
    let text = std::fs::read_to_string(&p).unwrap();
    assert!(
        text.contains(r#"[repos."my-org/app"]"#),
        "unexpected file:\n{text}"
    );
    let mut again = Repos::load(&p).unwrap();
    let rule = again.rule("my-org/APP").expect("lost the rule");
    assert_eq!(rule.sync, Some(true));
    assert_eq!(rule.name, "app-mirror");
    again
        .edit("my-org/app", |x| *x = RepoRule::default())
        .unwrap();
    assert!(
        again.rule("my-org/app").is_none(),
        "an empty rule must be dropped"
    );
}

#[test]
fn repos_several_rules() {
    let d = TempDir::new();
    let p = repos_file(&d);
    let mut r = Repos::load(&p).unwrap();
    for (name, v) in [("a/one", true), ("a/two", false), ("b/three", true)] {
        r.edit(name, |x| {
            x.sync = Some(v);
            x.note = "n \"quoted\"".into();
        })
        .unwrap();
    }
    r.save().unwrap();
    let again = Repos::load(&p).expect("the file we wrote does not load");
    assert_eq!(again.rules.len(), 3);
    assert_eq!(again.rule("a/two").unwrap().sync, Some(false));
    assert_eq!(again.rule("a/one").unwrap().note, "n \"quoted\"");
    let text = std::fs::read_to_string(&p).unwrap();
    assert_eq!(
        text.matches("[repos]").count(),
        0,
        "stray [repos] header:\n{text}"
    );
}

#[test]
fn repos_validation() {
    let d = TempDir::new();
    let p = repos_file(&d);
    for (name, body) in [
        ("bad name", "[repos.\"a/b\"]\nname = \"no spaces\"\n"),
        (
            "bad on_delete",
            "[repos.\"a/b\"]\non_delete = \"explode\"\n",
        ),
        ("unknown key", "[repos.\"a/b\"]\nsyncc = true\n"),
        ("unknown table", "[other]\nx = 1\n"),
    ] {
        std::fs::write(&p, body).unwrap();
        assert!(Repos::load(&p).is_err(), "{name} must be rejected");
    }
    std::fs::write(&p, "[repos.\"a/b\"]\nsyncc = true\n").unwrap();
    let err = Repos::load(&p).err().unwrap().to_string();
    assert!(err.ends_with(r#"repos."a/b".syncc"#), "{err}");
    let mut r = Repos::load(&repos_file(&TempDir::new())).unwrap();
    assert!(r.edit("a/b", |x| x.name = "bad name".into()).is_err());
    assert!(r.rules.is_empty());
}

const SAMPLE: &str = r#"# my config
[source]
type = "forgejo"
url = "https://git.example.com"
token = "abcdefghijkl"

[github]
token = "ghp_abcdefghijkl"

[accounts]
alice = "alice-gh"

[listen]
secret = "0123456789abcdef"

[sync]
# how often
poll_interval = 5
visibility = true

[filter]
include = [
  "a",
  "b",
]
skip_forks = false
"#;

fn sample_file(dir: &TempDir) -> String {
    write(dir, SAMPLE)
}

fn kv(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
    pairs
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect()
}

#[test]
fn set_values_keeps_everything_else() {
    let d = TempDir::new();
    let p = sample_file(&d);
    set_values(
        &p,
        &kv(&[
            ("sync.poll_interval", "30"),
            ("filter.new_repos", r#""review""#),
            ("filter.include", r#"["x"]"#),
            ("notify.url", r#""https://ntfy.example.com/t""#),
        ]),
    )
    .unwrap();
    let out = std::fs::read_to_string(&p).unwrap();
    for want in [
        "# my config",
        "# how often",
        "poll_interval = 30",
        "visibility = true",
        r#"new_repos = "review""#,
        r#"include = ["x"]"#,
        "skip_forks = false",
        "[notify]",
        r#"url = "https://ntfy.example.com/t""#,
    ] {
        assert!(out.contains(want), "missing {want:?} in:\n{out}");
    }
    assert!(
        !out.contains("\"a\""),
        "the old multi-line array was not replaced:\n{out}"
    );
    #[cfg(unix)]
    assert_eq!(owner::mode(&std::fs::metadata(&p).unwrap()), 0o600);
    let c = load(&p).unwrap();
    assert_eq!(c.filter.new_repos, "review");
    assert_eq!(c.sync.poll_interval, 30);
}

#[test]
fn set_values_keeps_crlf() {
    let d = TempDir::new();
    let p = write(&d, &SAMPLE.replace('\n', "\r\n"));
    set_values(&p, &kv(&[("sync.workers", "3")])).unwrap();
    let out = std::fs::read_to_string(&p).unwrap();
    assert!(out.contains("workers = 3\r\n"));
    assert!(!out.replace("\r\n", "").contains('\n'));
}

#[test]
fn set_values_refuses_invalid() {
    let d = TempDir::new();
    let p = sample_file(&d);
    let err = set_values(&p, &kv(&[("filter.new_repos", r#""maybe""#)])).unwrap_err();
    assert!(
        err.to_string().starts_with(
            "that would make the config invalid, nothing was changed: filter.new_repos"
        ),
        "{err}"
    );
    assert_eq!(
        std::fs::read_to_string(&p).unwrap(),
        SAMPLE,
        "the file must be untouched after a refusal"
    );
    let leftovers = std::fs::read_dir(d.path()).unwrap().count();
    assert_eq!(leftovers, 1, "the temporary file must be removed");
}

#[test]
fn keys_lookup_and_literals() {
    let k = find_key("Sync.Poll_Interval").expect("lookup failed");
    assert_eq!(k.get(&Config::default()), "5");
    assert!(
        k.literal(&["abc"]).is_err(),
        "a number key must refuse text"
    );
    assert_eq!(k.literal(&["+7"]).unwrap(), "7");
    let list = find_key("filter.exclude")
        .unwrap()
        .literal(&["a, b", "c"])
        .unwrap();
    assert_eq!(list, r#"["a", "b", "c"]"#);
    let b = find_key("sync.tags").unwrap();
    assert_eq!(b.literal(&["Yes"]).unwrap(), "true");
    assert!(b.literal(&["maybe"]).is_err());
    assert_eq!(
        find_key("notify.events").unwrap().get(&Config::default()),
        "pending, failing, pull_request"
    );
    assert_eq!(
        find_key("filter.topic")
            .unwrap()
            .literal(&["a \"b\""])
            .unwrap(),
        r#""a \"b\"""#
    );
    assert!(find_key("source.token").unwrap().secret);
    assert!(!find_key("source.token_file").unwrap().secret);
    assert_eq!(keys().len(), 46);
    for p in PROFILES {
        for (key, _) in p.set {
            assert!(
                find_key(key).is_some(),
                "profile {} sets unknown key {key}",
                p.name
            );
        }
    }
    assert_eq!(find_profile("TEAM").unwrap().name, "team");
    assert!(find_profile("teams").is_none());
    assert!(
        find_profile("personal")
            .unwrap()
            .describe()
            .starts_with("personal  Mirror")
    );
}

#[test]
fn every_key_reads_its_own_field() {
    for k in keys() {
        assert!(keys::has_value(k.name), "{} has no accessor", k.name);
    }
    let mut c = Config::default();
    c.pulls.message = "msg-value".into();
    c.paths.repos_file = "rf-value".into();
    assert_eq!(
        find_key("pull_requests.message").unwrap().get(&c),
        "msg-value"
    );
    assert_eq!(find_key("paths.repos_file").unwrap().get(&c), "rf-value");
}

#[cfg(unix)]
#[test]
fn set_values_keeps_owner() {
    if unsafe { libc::geteuid() } != 0 {
        return;
    }
    let d = TempDir::new();
    let p = sample_file(&d);
    std::os::unix::fs::chown(&p, Some(4242), Some(4343)).unwrap();
    set_values(&p, &kv(&[("sync.poll_interval", "9")])).unwrap();
    assert_eq!(owner::owner(&std::fs::metadata(&p).unwrap()), (4242, 4343));
}

#[cfg(unix)]
#[test]
fn new_files_take_the_owner_of_their_dir_as_root() {
    if unsafe { libc::geteuid() } != 0 {
        return;
    }
    let d = TempDir::new();
    std::os::unix::fs::chown(d.path(), Some(4242), Some(4343)).unwrap();
    let path = d.path().join("repos.toml").to_string_lossy().into_owned();
    let mut r = Repos::load(&path).unwrap();
    r.edit("alice/a", |x| x.sync = Some(true)).unwrap();
    r.save().unwrap();
    assert_eq!(
        owner::owner(&std::fs::metadata(&path).unwrap()),
        (4242, 4343)
    );
    let tree = d.path().join("repos").join("1.git").join("objects");
    std::fs::create_dir_all(&tree).unwrap();
    std::fs::write(tree.join("x"), b"x").unwrap();
    let outside = TempDir::new();
    std::fs::write(outside.path().join("secret"), b"s").unwrap();
    std::os::unix::fs::symlink(outside.path(), tree.join("link")).unwrap();
    tree_like_dir(&d.path().join("repos"), d.path());
    assert_eq!(
        owner::owner(&std::fs::metadata(tree.join("x")).unwrap()),
        (4242, 4343)
    );
    assert_eq!(
        owner::owner(&std::fs::symlink_metadata(tree.join("link")).unwrap()),
        (4242, 4343)
    );
    assert_eq!(
        owner::owner(&std::fs::metadata(outside.path().join("secret")).unwrap()),
        (0, 0)
    );
    assert_eq!(
        owner::owner(&std::fs::metadata(outside.path()).unwrap()),
        (0, 0)
    );

    let other = d.path().join("other");
    std::os::unix::fs::symlink(outside.path(), &other).unwrap();
    tree_like_dir(&other.join("secret"), d.path());
    assert_eq!(
        owner::owner(&std::fs::metadata(outside.path().join("secret")).unwrap()),
        (0, 0)
    );
}

#[test]
fn parse_errors_never_show_the_offending_line() {
    let d = TempDir::new();
    let body = MINIMAL.replacen(
        r#"token = "gh-token-123456""#,
        r#"token = "ghp_SECRETVALUE\q""#,
        1,
    );
    let path = write(&d, &body);
    let err = load(&path).err().unwrap().to_string();
    assert!(!err.contains("SECRETVALUE"), "{err}");
    assert!(
        err.starts_with(&format!("{path}: line 6, column ")),
        "{err}"
    );
    assert!(err.contains(r#"(last key "github.token")"#), "{err}");
    assert!(err.contains("single quotes"), "{err}");
    let body = MINIMAL.replacen(
        r#"secret = "s3cret-value-123""#,
        r#"secret = "s3cret-UNTERMINATED"#,
        1,
    );
    let err = load(&write(&d, &body)).err().unwrap().to_string();
    assert!(!err.contains("UNTERMINATED"), "{err}");
    assert!(err.contains("line 10, column "), "{err}");
    assert!(!err.contains('\n'), "{err}");
}

#[test]
fn keys_match_case_insensitively_like_go() {
    let d = TempDir::new();
    let body = r#"
[Source]
URL = "https://git.example.com"
Token = "src-token-123456"
Type = "gitea"
[GITHUB]
token = "gh-token-123456"
[Accounts]
Alice = "alice-gh"
[listen]
Secret = "s3cret-value-123"
[Sync]
Poll_Interval = 7
[Pull_Requests]
Mode = "comment"
"#;
    let c = load(&write(&d, body)).unwrap();
    assert_eq!(c.source.kind, "gitea");
    assert_eq!(c.source.url, "https://git.example.com");
    assert_eq!(c.sync.poll_interval, 7);
    assert_eq!(c.pulls.mode, "comment");
    assert_eq!(
        c.accounts.get("Alice").map(String::as_str),
        Some("alice-gh")
    );
    let both = format!("{MINIMAL}\n[sync]\npoll_interval = 3\nPoll_Interval = 4\n");
    let path = write(&d, &both);
    let err = load(&path).err().unwrap().to_string();
    assert_eq!(
        err,
        format!("unknown option(s) in {path}: sync.Poll_Interval")
    );
    let p = repos_file(&d);
    std::fs::write(
        &p,
        "[Repos.\"Alice/Site\"]\nSync = false\nName = \"site2\"\n",
    )
    .unwrap();
    let r = Repos::load(&p).unwrap();
    let rule = r.rule("alice/site").unwrap();
    assert_eq!(rule.sync, Some(false));
    assert_eq!(rule.name, "site2");
}

#[cfg(unix)]
#[test]
fn temporary_copies_are_private_from_the_start() {
    let d = TempDir::new();
    let dir = d.path().to_string_lossy().into_owned();
    let (tmp, _file) = owner::TempFile::create(&dir, ".config-", ".toml").unwrap();
    let st = std::fs::metadata(tmp.path()).unwrap();
    assert_eq!(owner::mode(&st), 0o600);
}

#[test]
fn debug_output_hides_secrets() {
    let d = TempDir::new();
    let c = load(&write(&d, MINIMAL)).unwrap();
    let shown = format!("{:?} {:?} {:?}", c.source, c.github, c.listen);
    for secret in ["src-token-123456", "gh-token-123456", "s3cret-value-123"] {
        assert!(!shown.contains(secret), "{shown}");
    }
    assert!(shown.contains("[hidden]"));
}

#[test]
fn secret_files_need_not_be_utf8() {
    let d = TempDir::new();
    let tok = d.path().join("tok");
    std::fs::write(&tok, b"tok-\xff-value\n").unwrap();
    let body = MINIMAL.replacen(
        r#"token = "gh-token-123456""#,
        &format!("token_file = {:?}", tok.to_string_lossy()),
        1,
    );
    let c = load(&write(&d, &body)).unwrap();
    assert_eq!(c.github.token, "tok-\u{fffd}-value");
}

#[test]
fn secrets_with_control_characters_are_refused() {
    let d = TempDir::new();
    let path = write(&d, MINIMAL);
    let env = |k: &str| (k == "GITSYNC_GITHUB_TOKEN").then(|| "env-token-123\r\n".to_string());
    let c = Config::load_with(&path, &env).unwrap();
    assert_eq!(c.github.token, "env-token-123");
    let body = MINIMAL.replacen(
        r#"token = "src-token-123456""#,
        r#"token = "src-token\r123456""#,
        1,
    );
    let err = load(&write(&d, &body)).err().unwrap();
    assert!(err.is_config());
    assert_eq!(err.to_string(), "source.token contains control characters");
    let fresh = TempDir::new();
    let path = write(&fresh, MINIMAL);
    let env = |k: &str| (k == "GITSYNC_WEBHOOK_SECRET").then(|| "sec\u{7f}ret".to_string());
    let err = Config::load_with(&path, &env).err().unwrap();
    assert_eq!(err.to_string(), "listen.secret contains control characters");
}
