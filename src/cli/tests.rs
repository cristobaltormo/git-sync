use std::sync::Arc;

use super::inventory::{Item, ST_IGNORED, ST_SYNCED, ST_WAITING};
use super::tui::parse_keys;
use super::tui_model::{Key, Mode, Screen, TuiModel, VIEW_NAMES};
use crate::config::{Config, RepoRule};
use crate::engine::{Entry, Verdict};
use crate::forge::Repo;
use crate::testutil::TempDir;

fn mk(id: i64, name: &str, status: &'static str, verdict: Verdict, managed: bool) -> Item {
    Item {
        repo: Arc::new(Repo {
            id,
            owner: "alice".into(),
            name: name.into(),
            private: true,
            ..Repo::default()
        }),
        verdict,
        status,
        has: managed,
        entry: Entry {
            managed,
            gh_owner: "alice-gh".into(),
            gh_name: name.into(),
            ..Entry::default()
        },
    }
}

fn test_model(dir: &TempDir) -> TuiModel {
    let items = vec![
        mk(
            1,
            "api",
            ST_SYNCED,
            Verdict {
                sync: true,
                ..Verdict::default()
            },
            true,
        ),
        mk(
            2,
            "new-thing",
            ST_WAITING,
            Verdict {
                pending: true,
                why: "waiting for your decision".into(),
                ..Verdict::default()
            },
            false,
        ),
        mk(
            3,
            "old",
            ST_IGNORED,
            Verdict {
                why: "ignored by you".into(),
                rule: Some(RepoRule {
                    sync: Some(false),
                    ..RepoRule::default()
                }),
                ..Verdict::default()
            },
            false,
        ),
    ];
    let cfg = Config {
        path: dir
            .path()
            .join("config.toml")
            .to_string_lossy()
            .into_owned(),
        ..Config::default()
    };
    let mut m = TuiModel::new(Arc::new(cfg), items);
    m.save = |_| Ok(());
    m
}

fn typed(m: &mut TuiModel, s: &str) {
    for k in parse_keys(s.as_bytes()) {
        m.press(k);
    }
}

#[test]
fn parse_keys_like_a_terminal() {
    let ks = parse_keys(b"\x1b[A\x1b[B\x1b[5~\x1bj \r\x7f\x03\x1b");
    assert_eq!(
        ks,
        vec![
            Key::Up,
            Key::Down,
            Key::PgUp,
            Key::Esc,
            Key::Rune('j'),
            Key::Rune(' '),
            Key::Enter,
            Key::Backspace,
            Key::CtrlC,
            Key::Esc
        ]
    );
    assert_eq!(parse_keys("ñ".as_bytes()), vec![Key::Rune('ñ')]);
}

#[test]
fn tui_starts_on_waiting_and_decides() {
    let dir = TempDir::new();
    let mut m = test_model(&dir);
    assert_eq!(VIEW_NAMES[m.view], "waiting");
    assert_eq!(m.visible().len(), 1);
    typed(&mut m, " ");
    assert_eq!(m.desired(1), 1);
    assert_eq!(m.changes(), 1);
    typed(&mut m, " ");
    assert_eq!(m.desired(1), 0);
    typed(&mut m, "r");
    assert_eq!(m.changes(), 0);
}

#[test]
fn tui_bulk_and_save() {
    let dir = TempDir::new();
    let mut m = test_model(&dir);
    typed(&mut m, "fff");
    typed(&mut m, "f");
    assert_eq!(VIEW_NAMES[m.view], "waiting");
    typed(&mut m, "A");
    assert_eq!(m.desired(1), 1);
    typed(&mut m, "w");
    assert_eq!(m.changes(), 0);
    assert!(m.msg.contains("Saved"), "no confirmation: {:?}", m.msg);
    assert_eq!(m.items[1].status, "queued");
}

#[test]
fn tui_quit_asks_when_dirty() {
    let dir = TempDir::new();
    let mut m = test_model(&dir);
    typed(&mut m, " q");
    assert!(
        !m.quit && m.mode == Mode::Confirm,
        "must ask before discarding"
    );
    typed(&mut m, "n");
    assert!(!m.quit, "n must keep the session");
    typed(&mut m, "q");
    typed(&mut m, "y");
    assert!(m.quit, "y must quit");
}

#[test]
fn tui_options_and_validation() {
    let dir = TempDir::new();
    let mut m = test_model(&dir);
    typed(&mut m, "o");
    typed(&mut m, "\x1b[B");
    typed(&mut m, "\r");
    typed(&mut m, "bad name");
    assert_eq!(m.prompt, "GitHub name (empty: same as the source)");
    typed(&mut m, "\r");
    assert!(
        m.changes() == 0 && m.msg_bad,
        "an invalid GitHub name must be refused (changes {}, msg {:?})",
        m.changes(),
        m.msg
    );
    assert!(m.msg.starts_with("name \"bad name\""), "{}", m.msg);
    typed(&mut m, "\r");
    typed(&mut m, "\x7f\x7f\x7f\x7f\x7f\x7f\x7f\x7f");
    typed(&mut m, "api-mirror\r");
    assert_eq!(m.rule(1).name, "api-mirror");
}

#[test]
fn tui_settings_stage_and_profile() {
    let dir = TempDir::new();
    let mut m = test_model(&dir);
    typed(&mut m, "\t");
    typed(&mut m, "\r");
    assert_eq!(
        m.staged.get("filter.new_repos").map(String::as_str),
        Some("\"review\"")
    );
    typed(&mut m, "G");
    typed(&mut m, "\r");
    assert_eq!(m.profile, "personal");
    typed(&mut m, "\r");
    assert_eq!(m.profile, "team");
    assert_eq!(
        m.staged.get("sync.on_delete").map(String::as_str),
        Some("\"archive\"")
    );
    assert_eq!(
        m.staged.get("filter.scope").map(String::as_str),
        Some("\"admin\"")
    );
}

#[test]
fn tui_renders_within_bounds() {
    let dir = TempDir::new();
    let mut m = test_model(&dir);
    for (w, h) in [(100, 30), (60, 12), (30, 10)] {
        for key in ["", "o", "\x1b", "\t", "?"] {
            typed(&mut m, key);
            let lines = m.render(w, h);
            assert_eq!(lines.len(), h as usize, "{w}x{h} after {key:?}");
            assert!(lines.iter().all(|l| !l.contains('\n')));
        }
        m.mode = Mode::List;
        m.screen = Screen::Repos;
    }
}

#[test]
fn tui_saves_to_disk() {
    let dir = TempDir::new();
    let path = dir.path().join("config.toml");
    std::fs::write(
        &path,
        "[source]\ntype = \"forgejo\"\nurl = \"https://git.example.com\"\ntoken = \"abcdefgh\"\n\n[github]\ntoken = \"ijklmnop\"\n\n[accounts]\nalice = \"alice-gh\"\n\n[listen]\nsecret = \"s3cret-value\"\n",
    )
    .unwrap();
    let cfg = Config::load(&path.to_string_lossy()).unwrap();
    let mut m = test_model(&dir);
    m.cfg = Arc::new(cfg);
    m.save = TuiModel::new(m.cfg.clone(), Vec::new()).save;
    typed(&mut m, " ");
    typed(&mut m, "\t\r");
    typed(&mut m, "w");
    assert!(!m.msg_bad, "{}", m.msg);
    let repos = std::fs::read_to_string(dir.path().join("repos.toml")).unwrap();
    assert!(
        repos.contains("[repos.\"alice/new-thing\"]\nsync = true\n"),
        "{repos}"
    );
    let saved = Config::load(&path.to_string_lossy()).unwrap();
    assert_eq!(saved.filter.new_repos, "review");
}

#[test]
fn repo_saves_build_on_what_is_on_disk() {
    let dir = TempDir::new();
    let path = dir.path().join("config.toml");
    std::fs::write(
        &path,
        "[source]\ntype = \"forgejo\"\nurl = \"https://git.example.com\"\ntoken = \"abcdefgh\"\n\n[github]\ntoken = \"ijklmnop\"\n\n[accounts]\nalice = \"alice-gh\"\n\n[listen]\nsecret = \"s3cret-value\"\n",
    )
    .unwrap();
    let cfg = Config::load(&path.to_string_lossy()).unwrap();
    for name in ["alice/a", "alice/b"] {
        super::repos::save_repos(&cfg, |r| r.edit(name, |x| x.sync = Some(true))).unwrap();
    }
    let repos = std::fs::read_to_string(dir.path().join("repos.toml")).unwrap();
    assert!(repos.contains("[repos.\"alice/a\"]"), "{repos}");
    assert!(repos.contains("[repos.\"alice/b\"]"), "{repos}");
}
