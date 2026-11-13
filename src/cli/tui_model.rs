use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;

use super::inventory::{Item, ST_ERROR, ST_FILTERED, ST_IGNORED, ST_QUEUED, ST_SYNCED, ST_WAITING};
use super::repos::save_repos;
use crate::config::{
    Config, PROFILES, RepoRule, Repos, find_key, find_profile, is_http_url, set_values,
};
use crate::error::Error;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Key {
    Rune(char),
    Up,
    Down,
    Left,
    Right,
    PgUp,
    PgDn,
    Home,
    End,
    Enter,
    Esc,
    Tab,
    Backspace,
    CtrlC,
}

pub(crate) struct Setting {
    pub(crate) name: &'static str,
    pub(crate) label: &'static str,
    pub(crate) help: &'static str,
    pub(crate) choices: &'static [&'static str],
    pub(crate) text: bool,
}

const fn choice(
    name: &'static str,
    label: &'static str,
    help: &'static str,
    choices: &'static [&'static str],
) -> Setting {
    Setting {
        name,
        label,
        help,
        choices,
        text: false,
    }
}

const fn text(name: &'static str, label: &'static str, help: &'static str) -> Setting {
    Setting {
        name,
        label,
        help,
        choices: &[],
        text: true,
    }
}

pub(crate) static SETTING_ROWS: &[Setting] = &[
    choice(
        "filter.new_repos",
        "New repositories",
        "sync: start right away. review: wait for your OK. ignore: only what you pick.",
        &["sync", "review", "ignore"],
    ),
    choice(
        "filter.scope",
        "Which repos count",
        "all, or only those where you are an administrator (Forgejo, Gitea, Gogs).",
        &["all", "admin"],
    ),
    choice("filter.skip_forks", "Skip forks", "", &["false", "true"]),
    choice(
        "filter.skip_private",
        "Skip private repos",
        "",
        &["false", "true"],
    ),
    choice(
        "filter.skip_archived",
        "Skip archived repos",
        "",
        &["false", "true"],
    ),
    choice(
        "filter.skip_mirrors",
        "Skip pull mirrors",
        "",
        &["true", "false"],
    ),
    choice(
        "sync.visibility",
        "Follow visibility",
        "Public on the source becomes public on GitHub. Private never becomes public.",
        &["true", "false"],
    ),
    choice("sync.tags", "Mirror tags", "", &["true", "false"]),
    choice(
        "sync.metadata",
        "Copy description and topics",
        "",
        &["true", "false"],
    ),
    choice(
        "sync.on_delete",
        "When a repo is deleted",
        "What happens to the GitHub copy.",
        &["delete", "archive", "ignore"],
    ),
    choice(
        "pull_requests.mode",
        "Pull requests on mirrors",
        "leave: do nothing. comment: add one note. They are never merged or closed.",
        &["leave", "comment"],
    ),
    text(
        "notify.url",
        "Notification webhook",
        "Receives a message when a repo is waiting, fails or gets a pull request.",
    ),
    text(
        "sync.poll_interval",
        "Poll interval (seconds)",
        "How often the source is checked. 0 relies on webhooks only.",
    ),
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Screen {
    Repos,
    Settings,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Mode {
    List,
    Search,
    Options,
    Input,
    Confirm,
    Help,
}

pub(crate) const VIEW_NAMES: [&str; 4] = ["all", "waiting", "synced", "skipped"];

#[derive(Debug, Clone, Copy)]
enum Pending {
    RepoName(usize),
    RepoNote(usize),
    Setting(usize),
}

pub(crate) type SaveFn = fn(&TuiModel) -> Result<(), Error>;
pub(crate) type RefreshFn = Box<dyn FnMut() -> Result<(Arc<Config>, Vec<Item>), Error>>;

pub(crate) struct TuiModel {
    pub(crate) cfg: Arc<Config>,
    pub(crate) items: Vec<Item>,
    orig: HashMap<String, RepoRule>,
    pub(crate) draft: BTreeMap<String, RepoRule>,
    pub(crate) staged: BTreeMap<String, String>,
    pub(crate) screen: Screen,
    pub(crate) mode: Mode,
    pub(crate) view: usize,
    pub(crate) query: String,
    pub(crate) cursor: isize,
    pub(crate) top: isize,
    pub(crate) opt: isize,
    pub(crate) input: String,
    pub(crate) prompt: String,
    pending: Option<Pending>,
    pub(crate) msg: String,
    pub(crate) msg_bad: bool,
    pub(crate) quit: bool,
    pub(crate) save: SaveFn,
    pub(crate) profile: &'static str,
    pub(crate) refresh: Option<RefreshFn>,
    pub(crate) height: isize,
    pub(crate) width: isize,
}

fn lower(s: &str) -> String {
    s.to_lowercase()
}

fn originals(items: &[Item]) -> HashMap<String, RepoRule> {
    items
        .iter()
        .filter_map(|it| {
            it.verdict
                .rule
                .as_ref()
                .map(|r| (lower(&it.full()), r.clone()))
        })
        .collect()
}

impl TuiModel {
    pub(crate) fn new(cfg: Arc<Config>, items: Vec<Item>) -> TuiModel {
        let mut m = TuiModel {
            orig: originals(&items),
            cfg,
            items,
            draft: BTreeMap::new(),
            staged: BTreeMap::new(),
            screen: Screen::Repos,
            mode: Mode::List,
            view: 0,
            query: String::new(),
            cursor: 0,
            top: 0,
            opt: 0,
            input: String::new(),
            prompt: String::new(),
            pending: None,
            msg: String::new(),
            msg_bad: false,
            quit: false,
            save: save_tui,
            profile: "",
            refresh: None,
            height: 30,
            width: 100,
        };
        let n = m.pending_count();
        if n > 0 {
            m.view = 1;
            let noun = if n == 1 { "y is" } else { "ies are" };
            m.say(format!(
                "{n} new repositor{noun} waiting: space picks, A syncs all shown, I ignores all shown, w saves"
            ));
        }
        m
    }

    pub(crate) fn rule(&self, i: usize) -> RepoRule {
        let k = lower(&self.items[i].full());
        if let Some(d) = self.draft.get(&k) {
            return d.clone();
        }
        self.orig.get(&k).cloned().unwrap_or_default()
    }

    fn edit(&mut self, i: usize, f: impl FnOnce(&mut RepoRule)) {
        let k = lower(&self.items[i].full());
        let mut d = match self.draft.get(&k) {
            Some(d) => d.clone(),
            None => self.orig.get(&k).cloned().unwrap_or_default(),
        };
        f(&mut d);
        let same = match self.orig.get(&k) {
            Some(o) => *o == d,
            None => d.is_empty(),
        };
        if same {
            self.draft.remove(&k);
        } else {
            self.draft.insert(k, d);
        }
    }

    pub(crate) fn dirty(&self, i: usize) -> bool {
        self.draft.contains_key(&lower(&self.items[i].full()))
    }

    pub(crate) fn changes(&self) -> usize {
        self.draft.len() + self.staged.len()
    }

    fn pending_count(&self) -> usize {
        self.items
            .iter()
            .filter(|it| it.status == ST_WAITING)
            .count()
    }

    pub(crate) fn desired(&self, i: usize) -> i8 {
        let r = self.rule(i);
        if let Some(s) = r.sync {
            return i8::from(s);
        }
        let it = &self.items[i];
        if self.dirty(i) || it.verdict.rule.as_ref().is_some_and(|x| x.sync.is_some()) {
            return -1;
        }
        i8::from(it.verdict.sync)
    }

    pub(crate) fn visible(&self) -> Vec<usize> {
        let q = lower(&self.query);
        self.items
            .iter()
            .enumerate()
            .filter(|(_, it)| {
                if !q.is_empty() && !lower(&it.full()).contains(&q) {
                    return false;
                }
                match VIEW_NAMES[self.view] {
                    "waiting" => it.status == ST_WAITING,
                    "synced" => matches!(it.status, ST_SYNCED | ST_ERROR | ST_QUEUED),
                    "skipped" => matches!(it.status, ST_IGNORED | ST_FILTERED),
                    _ => true,
                }
            })
            .map(|(i, _)| i)
            .collect()
    }

    pub(crate) fn current(&mut self) -> Option<usize> {
        let v = self.visible();
        if v.is_empty() {
            return None;
        }
        self.cursor = self.cursor.clamp(0, v.len() as isize - 1);
        Some(v[self.cursor as usize])
    }

    fn set_sync(&mut self, i: usize, v: Option<bool>) {
        self.edit(i, |r| r.sync = v);
    }

    fn say(&mut self, msg: String) {
        self.msg = msg;
        self.msg_bad = false;
    }

    fn warn(&mut self, msg: String) {
        self.msg = msg;
        self.msg_bad = true;
    }

    pub(crate) fn press(&mut self, k: Key) {
        self.msg.clear();
        match self.mode {
            Mode::Search => self.press_search(k),
            Mode::Options => self.press_options(k),
            Mode::Input => self.press_input(k),
            Mode::Confirm => {
                self.mode = Mode::List;
                if matches!(k, Key::Rune('y' | 'Y')) {
                    self.quit = true;
                }
            }
            Mode::Help => self.mode = Mode::List,
            Mode::List => {
                if self.screen == Screen::Settings {
                    self.press_settings(k);
                } else {
                    self.press_list(k);
                }
            }
        }
    }

    fn try_quit(&mut self) {
        if self.changes() == 0 {
            self.quit = true;
            return;
        }
        self.mode = Mode::Confirm;
    }

    fn move_cursor(&mut self, k: Key, n: usize) -> bool {
        let n = n as isize;
        let page = (self.height - 9).max(3);
        match k {
            Key::Up | Key::Rune('k') => self.cursor -= 1,
            Key::Down | Key::Rune('j') => self.cursor += 1,
            Key::PgUp => self.cursor -= page,
            Key::PgDn => self.cursor += page,
            Key::Home | Key::Rune('g') => self.cursor = 0,
            Key::End | Key::Rune('G') => self.cursor = n - 1,
            _ => return false,
        }
        self.cursor = self.cursor.min((n - 1).max(0)).max(0);
        true
    }

    fn press_list(&mut self, k: Key) {
        let vis = self.visible();
        if self.move_cursor(k, vis.len()) {
            return;
        }
        let it = self.current();
        match k {
            Key::CtrlC | Key::Esc => {
                if k == Key::Esc && !self.query.is_empty() {
                    self.query.clear();
                    return;
                }
                self.try_quit();
            }
            Key::Tab => {
                self.screen = Screen::Settings;
                self.cursor = 0;
            }
            Key::Enter => {
                if it.is_some() {
                    self.mode = Mode::Options;
                    self.opt = 0;
                }
            }
            Key::Rune(r) => self.press_list_rune(r, it, &vis),
            _ => {}
        }
    }

    fn press_list_rune(&mut self, r: char, it: Option<usize>, vis: &[usize]) {
        match r {
            'q' => self.try_quit(),
            ' ' => {
                let Some(i) = it else { return };
                let next = self.desired(i) != 1;
                self.set_sync(i, Some(next));
                if self.cursor < vis.len() as isize - 1 && VIEW_NAMES[self.view] == "all" {
                    self.cursor += 1;
                }
            }
            'A' => {
                for &x in vis {
                    self.set_sync(x, Some(true));
                }
                self.say(format!("{} repositories will be synced", vis.len()));
            }
            'a' => {
                if let Some(i) = it {
                    self.set_sync(i, Some(true));
                }
            }
            'I' => {
                for &x in vis {
                    self.set_sync(x, Some(false));
                }
                self.say(format!("{} repositories will be ignored", vis.len()));
            }
            'i' => {
                if let Some(i) = it {
                    self.set_sync(i, Some(false));
                }
            }
            'r' => {
                if let Some(i) = it {
                    self.set_sync(i, None);
                }
            }
            'f' => {
                self.view = (self.view + 1) % VIEW_NAMES.len();
                self.cursor = 0;
                self.top = 0;
            }
            '/' => self.mode = Mode::Search,
            'o' => {
                if it.is_some() {
                    self.mode = Mode::Options;
                    self.opt = 0;
                }
            }
            'w' | 's' => self.do_save(),
            '?' => self.mode = Mode::Help,
            _ => {}
        }
    }

    fn press_search(&mut self, k: Key) {
        match k {
            Key::Esc => {
                self.query.clear();
                self.mode = Mode::List;
            }
            Key::Enter | Key::Up | Key::Down | Key::CtrlC => self.mode = Mode::List,
            Key::Backspace => {
                self.query.pop();
            }
            Key::Rune(r) => self.query.push(r),
            _ => {}
        }
        self.cursor = 0;
        self.top = 0;
    }

    pub(crate) fn options(&self, i: usize) -> Vec<(&'static str, String)> {
        let r = self.rule(i);
        let tri = |p: Option<bool>| match p {
            None => "default",
            Some(true) => "yes",
            Some(false) => "no",
        };
        let or = |s: &str, def: &str| {
            if s.is_empty() {
                def.to_string()
            } else {
                s.to_string()
            }
        };
        let sync = match self.desired(i) {
            1 => "yes",
            0 => "no",
            _ => "as the general rules say",
        };
        vec![
            ("Sync this repository", sync.to_string()),
            ("Name on GitHub", or(&r.name, "same as the source")),
            ("Keep the copy private", tri(r.keep_private).into()),
            ("Mirror tags", tri(r.tags).into()),
            ("Copy description and topics", tri(r.metadata).into()),
            ("Delete branches only on GitHub", tri(r.prune).into()),
            ("When removed from the source", or(&r.on_delete, "default")),
            ("Note", or(&r.note, "-")),
        ]
    }

    fn press_options(&mut self, k: Key) {
        let Some(i) = self.current() else {
            self.mode = Mode::List;
            return;
        };
        let rows = self.options(i).len() as isize;
        match k {
            Key::Esc | Key::CtrlC | Key::Rune('q') => {
                self.mode = Mode::List;
                return;
            }
            Key::Up | Key::Rune('k') => self.opt -= 1,
            Key::Down | Key::Tab | Key::Rune('j') => self.opt += 1,
            Key::Rune(' ') | Key::Enter | Key::Right => self.change_option(i, false),
            Key::Left => self.change_option(i, true),
            _ => {}
        }
        self.opt = (self.opt + rows) % rows;
    }

    fn change_option(&mut self, i: usize, back: bool) {
        match self.opt {
            0 => {
                let next = match (self.desired(i), back) {
                    (1, false) => Some(false),
                    (0, false) => None,
                    (_, false) => Some(true),
                    (1, true) => None,
                    (0, true) => Some(true),
                    (_, true) => Some(false),
                };
                self.set_sync(i, next);
            }
            1 => {
                let cur = self.rule(i).name;
                self.ask(
                    "GitHub name (empty: same as the source)",
                    cur,
                    Pending::RepoName(i),
                );
            }
            2 => self.edit(i, |r| r.keep_private = cycle_bool(r.keep_private, back)),
            3 => self.edit(i, |r| r.tags = cycle_bool(r.tags, back)),
            4 => self.edit(i, |r| r.metadata = cycle_bool(r.metadata, back)),
            5 => self.edit(i, |r| r.prune = cycle_bool(r.prune, back)),
            6 => {
                const CS: [&str; 4] = ["", "delete", "archive", "ignore"];
                self.edit(i, |r| {
                    r.on_delete = cycle_choice(&CS, &r.on_delete, back).to_string()
                });
            }
            7 => {
                let cur = self.rule(i).note;
                self.ask("Note", cur, Pending::RepoNote(i));
            }
            _ => {}
        }
    }

    fn apply(&mut self, i: usize, f: impl Fn(&mut RepoRule)) {
        let mut probe = self.rule(i);
        f(&mut probe);
        let full = self.items[i].full();
        if let Err(e) = validate_rule(&full, &probe) {
            let msg = e.to_string();
            let prefix = format!("repos.{full:?}: ");
            let msg = msg.strip_prefix(&prefix).unwrap_or(&msg).to_string();
            self.warn(msg);
            return;
        }
        self.edit(i, f);
    }

    fn ask(&mut self, prompt: &str, def: String, then: Pending) {
        self.mode = Mode::Input;
        self.input = def;
        self.pending = Some(then);
        self.prompt = prompt.to_string();
    }

    fn press_input(&mut self, k: Key) {
        match k {
            Key::Esc | Key::CtrlC => {
                self.mode = self.return_mode();
                self.pending = None;
            }
            Key::Enter => {
                let v = self.input.trim().to_string();
                self.mode = self.return_mode();
                if let Some(p) = self.pending.take() {
                    self.finish_input(p, v);
                }
            }
            Key::Backspace => {
                self.input.pop();
            }
            Key::Rune(r) => self.input.push(r),
            _ => {}
        }
    }

    fn finish_input(&mut self, p: Pending, v: String) {
        match p {
            Pending::RepoName(i) => self.apply(i, |r| r.name = v.clone()),
            Pending::RepoNote(i) => self.apply(i, |r| r.note = v.clone()),
            Pending::Setting(s) => {
                let s = &SETTING_ROWS[s];
                let Some(key) = find_key(s.name) else { return };
                let lit = match key.literal(&[v.as_str()]) {
                    Ok(l) => l,
                    Err(e) => return self.warn(e.to_string()),
                };
                if !v.is_empty() && s.name == "notify.url" && !is_http_url(&v) {
                    return self.warn("that is not an http(s) URL".into());
                }
                self.stage(s.name, lit);
            }
        }
    }

    fn return_mode(&self) -> Mode {
        if self.screen == Screen::Settings {
            Mode::List
        } else {
            Mode::Options
        }
    }

    pub(crate) fn setting_value(&self, s: &Setting) -> String {
        if let Some(lit) = self.staged.get(s.name) {
            return lit.trim_matches('"').to_string();
        }
        find_key(s.name)
            .map(|k| k.get(&self.cfg))
            .unwrap_or_default()
    }

    fn press_settings(&mut self, k: Key) {
        let n = SETTING_ROWS.len() + 1;
        if self.move_cursor(k, n) {
            return;
        }
        match k {
            Key::CtrlC | Key::Esc | Key::Rune('q') => self.try_quit(),
            Key::Tab => {
                self.screen = Screen::Repos;
                self.cursor = 0;
            }
            Key::Left => self.change_setting(true),
            Key::Enter | Key::Right | Key::Rune(' ') => self.change_setting(false),
            Key::Rune('w' | 's') => self.do_save(),
            Key::Rune('?') => self.mode = Mode::Help,
            _ => {}
        }
    }

    fn stage(&mut self, name: &str, lit: String) {
        if lit == self.literal_of(name) {
            self.staged.remove(name);
            return;
        }
        self.staged.insert(name.to_string(), lit);
    }

    fn literal_of(&self, name: &str) -> String {
        find_key(name)
            .and_then(|k| k.literal(&[k.get(&self.cfg)]).ok())
            .unwrap_or_default()
    }

    fn change_setting(&mut self, back: bool) {
        let cursor = self.cursor.max(0) as usize;
        if cursor >= SETTING_ROWS.len() {
            let mut names = vec![""];
            names.extend(PROFILES.iter().map(|p| p.name));
            self.profile = cycle_choice(&names, self.profile, back);
            if let Some(p) = find_profile(self.profile) {
                for (k, v) in p.set {
                    self.stage(k, v.to_string());
                }
                self.say(format!(
                    "Profile {} staged: review the settings above, then save",
                    p.name
                ));
            }
            return;
        }
        let s = &SETTING_ROWS[cursor];
        let cur = self.setting_value(s);
        if s.text {
            self.ask(s.label, cur, Pending::Setting(cursor));
            return;
        }
        let next = cycle_choice(s.choices, &cur, back);
        if let Some(lit) = find_key(s.name).and_then(|k| k.literal(&[next]).ok()) {
            self.stage(s.name, lit);
        }
    }

    pub(crate) fn do_save(&mut self) {
        if self.changes() == 0 {
            self.say("Nothing to save".into());
            return;
        }
        let n = self.changes();
        let save = self.save;
        if let Err(e) = save(self) {
            self.warn(e.to_string());
            return;
        }
        let refreshed = self.refresh.as_mut().map(|f| f());
        if let Some(result) = refreshed {
            match result {
                Err(e) => {
                    self.warn(format!("saved, but could not refresh the list: {e}"));
                    self.after_save();
                    return;
                }
                Ok((cfg, items)) => {
                    self.cfg = cfg;
                    self.items = items;
                    self.draft.clear();
                    self.staged.clear();
                    self.orig = originals(&self.items);
                }
            }
        } else {
            self.after_save();
        }
        self.say(format!(
            "Saved {n} change(s). A running gitsync applies them on its own."
        ));
    }

    fn after_save(&mut self) {
        let draft = std::mem::take(&mut self.draft);
        for it in &mut self.items {
            let k = lower(&it.full());
            let Some(c) = draft.get(&k) else { continue };
            if c.is_empty() {
                self.orig.remove(&k);
                it.verdict.rule = None;
            } else {
                self.orig.insert(k, c.clone());
                it.verdict.rule = Some(c.clone());
            }
            match c.sync {
                Some(true) => {
                    it.status = if it.managed() { ST_SYNCED } else { ST_QUEUED };
                    it.verdict.sync = true;
                    it.verdict.pending = false;
                }
                Some(false) => {
                    it.status = ST_IGNORED;
                    it.verdict.sync = false;
                    it.verdict.pending = false;
                    it.verdict.why = "ignored by you".into();
                }
                None => {}
            }
        }
        self.staged.clear();
    }
}

fn validate_rule(full: &str, r: &RepoRule) -> Result<(), Error> {
    Repos::default().edit(full, |x| *x = r.clone())
}

fn cycle_bool(p: Option<bool>, back: bool) -> Option<bool> {
    const STEPS: [Option<bool>; 3] = [None, Some(true), Some(false)];
    let i = STEPS.iter().position(|s| *s == p).unwrap_or(0);
    let i = if back { i + STEPS.len() - 1 } else { i + 1 };
    STEPS[i % STEPS.len()]
}

fn cycle_choice<'a>(choices: &[&'a str], cur: &str, back: bool) -> &'a str {
    let i = choices.iter().rposition(|c| *c == cur).unwrap_or(0);
    let i = if back { i + choices.len() - 1 } else { i + 1 };
    choices[i % choices.len()]
}

fn save_tui(m: &TuiModel) -> Result<(), Error> {
    if !m.staged.is_empty() {
        set_values(&m.cfg.path, &m.staged)?;
    }
    if !m.draft.is_empty() {
        save_repos(&m.cfg, |r| {
            for (k, d) in &m.draft {
                r.edit(k, |x| *x = d.clone())?;
            }
            Ok(())
        })?;
    }
    Ok(())
}
