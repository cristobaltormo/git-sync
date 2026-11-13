use super::*;
use crate::forge::{Event, Hooks, Identity, Source};
use crate::httpx::Headers;
use crate::testutil::TempDir;

struct Fake {
    header: String,
    local: bool,
}

impl Source for Fake {
    fn kind(&self) -> &'static str {
        "fake"
    }
    fn whoami(&self) -> Result<Identity, Error> {
        Ok(Identity::default())
    }
    fn list(&self, _: &str) -> Result<Vec<Arc<Repo>>, Error> {
        Ok(Vec::new())
    }
    fn get(&self, _: &str, _: &str) -> Result<Option<Arc<Repo>>, Error> {
        Ok(None)
    }
    fn owner_exists(&self, _: &str) -> Result<bool, Error> {
        Ok(true)
    }
    fn hooks(&self) -> Option<&dyn Hooks> {
        None
    }
}

impl Provider for Fake {
    fn clone_url(&self, r: &Repo) -> String {
        if self.local {
            return r.website.clone();
        }
        format!("https://git.example.com/{}.git", r.full())
    }
    fn git_header(&self) -> Result<String, Error> {
        Ok(self.header.clone())
    }
    fn parse_webhook(&self, _: &Headers, _: &[u8], _: &str) -> Result<Option<Event>, Error> {
        Ok(None)
    }
}

fn fake(header: &str, local: bool) -> Arc<dyn Provider> {
    Arc::new(Fake {
        header: header.into(),
        local,
    })
}

#[test]
fn credentials_are_scoped_headers() {
    let mut cfg = Config::default();
    cfg.source.url = "https://git.example.com".into();
    cfg.source.token = "src-token-123456".into();
    cfg.github.token = "gh-token-123456".into();
    let g = Git::new(
        &cfg,
        fake("Authorization: token src-token-123456", false),
        false,
    );
    for a in GIT_OPTS {
        assert!(
            !a.contains("token"),
            "no credentials on the command line: {a}"
        );
    }
    let env = g.environment().unwrap();
    let joined: Vec<String> = env.iter().map(|(k, v)| format!("{k}={v}")).collect();
    for want in [
        "GIT_CONFIG_KEY_0=http.https://git.example.com/.extraheader",
        "GIT_CONFIG_VALUE_0=Authorization: token src-token-123456",
        "GIT_CONFIG_KEY_1=http.https://github.com/.extraheader",
        "GIT_TERMINAL_PROMPT=0",
        "GIT_CONFIG_COUNT=2",
    ] {
        assert!(joined.iter().any(|l| l == want), "missing {want:?}");
    }
    let gh = &env
        .iter()
        .find(|(k, _)| k == "GIT_CONFIG_VALUE_1")
        .unwrap()
        .1;
    assert!(!gh.contains("src-token-123456") && !gh.contains("gh-token-123456"));
    assert_eq!(
        gh,
        &format!(
            "Authorization: Basic {}",
            base64(b"x-access-token:gh-token-123456")
        )
    );
    assert_eq!(logx::clean(gh), "Authorization: Basic ***");
}

#[test]
fn tail_keeps_the_last_three_lines() {
    let mut t = Tail::new();
    t.write(b"one\ntwo\r");
    t.write(b"Writing objects: 50%\rthree\n  \nfour");
    assert_eq!(t.text(), "two | Writing objects: 50% | three");
    t.write(b"\n");
    assert_eq!(t.text(), "Writing objects: 50% | three | four");
}

#[cfg(unix)]
mod fake_git {
    use super::*;

    fn script(dir: &TempDir, body: &str) -> OsString {
        use std::os::unix::fs::PermissionsExt;
        let p = dir.path().join("git");
        std::fs::write(&p, format!("#!/bin/sh\n{body}\n")).unwrap();
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
        p.into_os_string()
    }

    fn test_git(program: OsString) -> Git {
        let mut g = Git::new(&Config::default(), fake("", false), false);
        g.stall = Duration::from_millis(1500);
        g.program = program;
        g
    }

    #[test]
    fn silent_git_is_killed() {
        let d = TempDir::new();
        let g = test_git(script(&d, "sleep 30"));
        let start = Instant::now();
        let err = g.run(&[], None, &["push"]).unwrap_err();
        assert!(
            matches!(&err, RunError::Stalled(m) if m == "git push: stalled for 1.5s"),
            "{err:?}"
        );
        assert!(
            start.elapsed() < Duration::from_secs(5),
            "a silent git must be stopped quickly"
        );
    }

    #[test]
    fn escaped_children_do_not_hang_the_worker() {
        let d = TempDir::new();
        let mut g = test_git(script(&d, "setsid sleep 8 & sleep 30"));
        g.wait_delay = Duration::from_millis(500);
        let start = Instant::now();
        let err = g.run(&[], None, &["fetch"]).unwrap_err();
        assert!(matches!(&err, RunError::Stalled(_)), "{err:?}");
        assert!(
            start.elapsed() < Duration::from_secs(5),
            "waited {:?} for a pipe held by an escaped child",
            start.elapsed()
        );
    }

    #[test]
    fn exits_right_at_the_stall_limit_are_reaped_once() {
        let d = TempDir::new();
        let mut g = test_git(script(&d, "sleep 1.4; echo late"));
        g.stall = Duration::from_millis(1400);
        for _ in 0..3 {
            match g.run(&[], None, &["fetch"]) {
                Ok(out) => assert_eq!(out.trim(), "late"),
                Err(e) => assert!(matches!(e, RunError::Stalled(_)), "{e:?}"),
            }
        }
    }

    #[test]
    fn mkdir_errors_name_the_path() {
        let d = TempDir::new();
        let file = d.path().join("state");
        std::fs::write(&file, "").unwrap();
        let mut g = test_git(script(&d, "exit 0"));
        g.root = file.join("repos");
        let r = Repo {
            id: 12,
            owner: "o".into(),
            name: "n".into(),
            ..Default::default()
        };
        let err = g
            .mirror(&r, "o", "n", "", Options::default())
            .unwrap_err()
            .to_string();
        assert!(
            err.starts_with("mkdir ") && err.contains("12.git: "),
            "{err}"
        );
    }

    #[test]
    fn busy_git_is_left_alone() {
        let d = TempDir::new();
        let g = test_git(script(
            &d,
            "i=1; while [ $i -le 8 ]; do echo \"Writing objects: $i\" >&2; sleep 0.25; i=$((i+1)); done; echo done",
        ));
        let out = g
            .run(&[], None, &["push"])
            .expect("a git that reports progress is fine");
        assert_eq!(out.trim(), "done");
    }

    #[test]
    fn stalled_git_is_retried() {
        let d = TempDir::new();
        let marker = d.path().join("seen");
        let body = format!(
            "if [ -e '{m}' ]; then echo ok; exit 0; fi; touch '{m}'; sleep 30",
            m = marker.display()
        );
        let g = test_git(script(&d, &body));
        let out = g
            .attempt(&[], None, &["push"])
            .expect("the second try should succeed");
        assert_eq!(out.trim(), "ok");
    }

    #[test]
    fn git_failure_shows_its_last_lines() {
        let d = TempDir::new();
        let g = test_git(script(
            &d,
            "printf 'one\\ntwo\\nthree\\nfatal: nope\\n' >&2; exit 1",
        ));
        let err = Error::from(g.run(&[], None, &["fetch"]).unwrap_err());
        assert_eq!(
            err.to_string(),
            "git fetch failed: two | three | fatal: nope"
        );
    }

    #[test]
    fn overall_timeout_stops_a_chatty_git() {
        let d = TempDir::new();
        let mut g = test_git(script(&d, "while true; do echo tick >&2; sleep 0.1; done"));
        g.timeout = Some(Duration::from_millis(600));
        let err = Error::from(g.run(&[], None, &["fetch"]).unwrap_err());
        assert_eq!(err.to_string(), "git fetch timed out");
    }

    #[test]
    fn missing_git_is_reported() {
        let d = TempDir::new();
        let g = test_git(d.path().join("nope").into_os_string());
        let err = Error::from(g.run(&[], None, &["fetch"]).unwrap_err());
        assert_eq!(err.to_string(), "git is not installed");
    }
}

fn git_in(dir: &Path, args: &[&str]) -> String {
    let out = Command::new("git")
        .args([
            "-c",
            "user.email=a@b.c",
            "-c",
            "user.name=t",
            "-c",
            "commit.gpgsign=false",
        ])
        .args(args)
        .current_dir(dir)
        .env("GIT_CONFIG_GLOBAL", DEV_NULL)
        .env("GIT_CONFIG_SYSTEM", DEV_NULL)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

struct Setup {
    _dir: TempDir,
    src: PathBuf,
    dst: PathBuf,
    root: PathBuf,
    git: Git,
    repo: Repo,
}

fn setup() -> Setup {
    let dir = TempDir::new();
    let root = dir.path().to_path_buf();
    let src = root.join("source");
    let gh = root.join("gh");
    let dst = gh.join("alice").join("site.git");
    std::fs::create_dir_all(gh.join("alice")).unwrap();
    git_in(&root, &["init", "-q", "-b", "main", src.to_str().unwrap()]);
    git_in(&src, &["commit", "-q", "--allow-empty", "-m", "one"]);
    git_in(&root, &["init", "-q", "--bare", dst.to_str().unwrap()]);
    let mut cfg = Config::default();
    cfg.paths.state_dir = root.join("state").to_string_lossy().into_owned();
    cfg.github.token = "gh-token-123456".into();
    cfg.github.git_url = format!("file://{}", gh.display());
    let git = Git::new(&cfg, fake("X-Test: 1", true), false);
    let repo = Repo {
        id: 7,
        owner: "alice".into(),
        name: "site".into(),
        website: src.to_string_lossy().into_owned(),
        ..Default::default()
    };
    Setup {
        _dir: dir,
        src,
        dst,
        root,
        git,
        repo,
    }
}

const ALL: Options = Options {
    tags: true,
    prune: true,
};

#[test]
fn mirror_pushes_then_skips_when_nothing_changed() {
    let s = setup();
    let first = s.git.mirror(&s.repo, "alice", "site", "", ALL).unwrap();
    assert_eq!(first.summary, "1 ref(s) updated");
    assert!(!first.refs.is_empty());
    let want = git_in(&s.src, &["rev-parse", "main"]);
    assert_eq!(git_in(&s.dst, &["rev-parse", "main"]), want);

    std::fs::remove_dir_all(&s.dst).unwrap();
    let again = s
        .git
        .mirror(&s.repo, "alice", "site", &first.refs, ALL)
        .unwrap();
    assert_eq!(again.summary, "up to date");
    assert_eq!(again.refs, first.refs);

    git_in(&s.src, &["commit", "-q", "--allow-empty", "-m", "two"]);
    git_in(&s.root, &["init", "-q", "--bare", s.dst.to_str().unwrap()]);
    let next = s
        .git
        .mirror(&s.repo, "alice", "site", &first.refs, ALL)
        .unwrap();
    assert_ne!(next.refs, first.refs);
    assert_ne!(next.summary, "up to date");

    s.git.forget(7);
    assert!(!s.root.join("state").join("repos").join("7.git").exists());
}

#[test]
fn mirror_keeps_branches_only_on_the_destination_when_prune_is_off() {
    let s = setup();
    let keep = Options {
        tags: true,
        prune: false,
    };
    s.git.mirror(&s.repo, "alice", "site", "", keep).unwrap();
    git_in(&s.dst, &["branch", "pr-head", "main"]);
    git_in(&s.src, &["commit", "-q", "--allow-empty", "-m", "two"]);
    s.git.mirror(&s.repo, "alice", "site", "", keep).unwrap();
    assert!(
        git_in(&s.dst, &["branch", "--list", "pr-head"]).contains("pr-head"),
        "a branch that exists only on the destination must survive when prune is off"
    );
    s.git.mirror(&s.repo, "alice", "site", "", ALL).unwrap();
    assert!(
        !git_in(&s.dst, &["branch", "--list", "pr-head"]).contains("pr-head"),
        "with prune on the extra branch must go"
    );
}

#[test]
fn mirror_reports_failures_and_dry_runs() {
    let s = setup();
    let gone = Repo {
        website: s.root.join("missing").to_string_lossy().into_owned(),
        ..s.repo.clone()
    };
    let err = s.git.mirror(&gone, "alice", "site", "", ALL).unwrap_err();
    assert!(err.to_string().starts_with("git fetch failed: "), "{err}");
    let mut cfg = Config::default();
    cfg.paths.state_dir = s.root.join("dry").to_string_lossy().into_owned();
    let dry = Git::new(&cfg, fake("", true), true);
    let out = dry.mirror(&s.repo, "alice", "site", "", ALL).unwrap();
    assert_eq!(out.summary, "dry-run");
    assert!(!s.root.join("dry").exists());
}
