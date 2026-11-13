use super::inventory::{
    Counts, Item, ST_ERROR, ST_FILTERED, ST_IGNORED, ST_QUEUED, ST_SYNCED, ST_WAITING,
};
use super::repos::{note, status_style};
use super::tui_model::{Mode, SETTING_ROWS, Screen, TuiModel, VIEW_NAMES};
use super::ui::{BOLD, DIM, GREEN, INVERT, RED, YELLOW, ago, clip, pad, paint, unix_now, width};

fn cursor_mark(on: bool) -> &'static str {
    if on { ">" } else { " " }
}

fn clip_to(s: &str, w: isize) -> String {
    clip(s, w.max(0) as usize)
}

fn pad_to(s: &str, w: isize) -> String {
    pad(s, w.max(0) as usize)
}

fn clip_styled(s: &str, w: isize) -> String {
    if !s.contains('\x1b') {
        return clip_to(s, w);
    }
    s.to_string()
}

impl TuiModel {
    pub(crate) fn render(&mut self, w: isize, h: isize) -> Vec<String> {
        self.width = w;
        self.height = h;
        let mut lines = self.header(w);
        let body = (h - 6).max(0);
        let mut rows = if self.mode == Mode::Help {
            help_lines()
        } else if self.mode == Mode::Options
            || (self.mode == Mode::Input && self.screen == Screen::Repos)
        {
            self.options_view(w)
        } else if self.screen == Screen::Settings {
            self.settings_view(w, body)
        } else {
            self.list_view(w, body)
        };
        rows.resize(body as usize, String::new());
        lines.extend(rows);
        lines.extend(self.footer(w));
        lines
    }

    fn header(&self, w: isize) -> Vec<String> {
        let (mut repos, mut sets) = (" Repositories ".to_string(), " Settings ".to_string());
        if self.screen == Screen::Repos {
            repos = paint(&repos, &[INVERT]);
        } else {
            sets = paint(&sets, &[INVERT]);
        }
        let mut title = format!("{} {repos}{sets}", paint(" gitsync ", &[BOLD]));
        let n = self.changes();
        if n > 0 {
            title.push_str("  ");
            title.push_str(&paint(&format!("{n} unsaved"), &[YELLOW, BOLD]));
        }
        let info = if self.screen == Screen::Repos {
            let c = Counts::of(&self.items);
            let summary = format!(
                " {} repositories: {} synced, {} waiting, {} skipped",
                self.items.len(),
                c.get(ST_SYNCED) + c.get(ST_ERROR) + c.get(ST_QUEUED),
                c.get(ST_WAITING),
                c.get(ST_IGNORED) + c.get(ST_FILTERED)
            );
            let mut info = format!(
                "{}   view: {}",
                paint(&summary, &[DIM]),
                paint(VIEW_NAMES[self.view], &[BOLD])
            );
            if self.mode == Mode::Search || !self.query.is_empty() {
                let cur = if self.mode == Mode::Search { "_" } else { "" };
                info.push_str("   search: ");
                info.push_str(&paint(&format!("{}{cur}", self.query), &[BOLD]));
            }
            info
        } else {
            paint(
                " How gitsync behaves. Everything here can be changed at any time; nothing is applied until you save.",
                &[DIM],
            )
        };
        vec![
            title,
            clip_styled(&info, w),
            paint(&"-".repeat(w.max(0) as usize), &[DIM]),
        ]
    }

    fn scroll(&mut self, n: isize, body: isize) {
        self.cursor = self.cursor.min(n - 1).max(0);
        if self.cursor < self.top {
            self.top = self.cursor;
        }
        if self.cursor >= self.top + body {
            self.top = self.cursor - body + 1;
        }
    }

    fn list_view(&mut self, w: isize, body: isize) -> Vec<String> {
        let vis = self.visible();
        if vis.is_empty() {
            let msg = if !self.query.is_empty() {
                " No repository matches the search."
            } else if VIEW_NAMES[self.view] == "waiting" {
                " Nothing is waiting for a decision. Press f to see the other views."
            } else {
                " Nothing to show here."
            };
            return vec![String::new(), paint(msg, &[DIM])];
        }
        self.scroll(vis.len() as isize, body);
        let name_w = vis
            .iter()
            .map(|&i| width(&self.items[i].full()) as isize)
            .fold(12, isize::max)
            .min((w / 3).max(18));
        let mut out = Vec::new();
        let start = self.top.max(0) as usize;
        let end = vis.len().min((self.top + body).max(0) as usize);
        for (row, &i) in vis.iter().enumerate().take(end).skip(start) {
            let it = &self.items[i];
            let box_ = match self.desired(i) {
                1 => "[x]",
                -1 => "[~]",
                _ => "[ ]",
            };
            let status = it.status;
            let mark = if self.dirty(i) { "*" } else { " " };
            let rest = w - 2 - 4 - name_w - 2 - 10 - 2;
            let extra = extra_text(it);
            let full = it.full();
            let name = pad_to(&clip_to(&full, name_w), name_w);
            let on = row as isize == self.cursor;
            if on {
                let plain = format!(
                    "{} {box_} {name}  {} {mark}  {}",
                    cursor_mark(true),
                    pad(status, 9),
                    clip_to(&extra, rest)
                );
                out.push(paint(&pad_to(&clip_to(&plain, w), w), &[INVERT]));
                continue;
            }
            out.push(format!(
                "{} {box_} {name}  {} {}  {}",
                cursor_mark(false),
                paint(&pad(status, 9), &[status_style(status)]),
                paint(mark, &[YELLOW, BOLD]),
                paint(&clip_to(&extra, rest), &[DIM])
            ));
        }
        out
    }

    fn options_view(&mut self, w: isize) -> Vec<String> {
        let Some(i) = self.current() else {
            return Vec::new();
        };
        let mut out = vec![
            paint(&format!(" Options for {}", self.items[i].full()), &[BOLD]),
            String::new(),
        ];
        for (n, (label, value)) in self.options(i).into_iter().enumerate() {
            let on = n as isize == self.opt;
            let line = format!(" {} {}  {value}", cursor_mark(on), pad(label, 30));
            if on {
                out.push(paint(&pad_to(&clip_to(&line, w), w), &[INVERT]));
            } else {
                out.push(line);
            }
        }
        out.push(String::new());
        out.push(paint(
            " The options override the general settings for this repository only.",
            &[DIM],
        ));
        out
    }

    fn settings_view(&mut self, w: isize, body: isize) -> Vec<String> {
        let n = SETTING_ROWS.len() as isize + 1;
        self.scroll(n, body);
        let mut out = Vec::new();
        let mut i = self.top;
        while i < n && i < self.top + body {
            let (mut label, mut value, mut staged) = (
                "Apply a ready-made profile",
                "choose with Enter".to_string(),
                false,
            );
            if (i as usize) < SETTING_ROWS.len() {
                let s = &SETTING_ROWS[i as usize];
                label = s.label;
                value = self.setting_value(s);
                staged = self.staged.contains_key(s.name);
                if s.text && value.is_empty() {
                    value = "(none)".into();
                }
            } else if !self.profile.is_empty() {
                value = format!("{} (staged)", self.profile);
            }
            let mark = if staged { "*" } else { " " };
            let on = i == self.cursor;
            let line = format!(
                " {} {} {} {mark}",
                cursor_mark(on),
                pad(label, 30),
                pad(&value, 22)
            );
            if on {
                out.push(paint(&pad_to(&clip_to(&line, w), w), &[INVERT]));
            } else if staged {
                out.push(paint(&clip_to(&line, w), &[YELLOW]));
            } else {
                out.push(clip_to(&line, w));
            }
            i += 1;
        }
        out
    }

    fn footer(&mut self, w: isize) -> Vec<String> {
        let on_setting = (self.cursor.max(0) as usize) < SETTING_ROWS.len();
        let detail = if self.screen == Screen::Settings && on_setting {
            format!(" {}", SETTING_ROWS[self.cursor.max(0) as usize].help)
        } else if self.screen == Screen::Settings {
            " personal: sync everything. team: new repos wait for you. careful: only what you pick."
                .to_string()
        } else if self.mode == Mode::List || self.mode == Mode::Search {
            match self.current() {
                Some(i) => format!(" {}", detail_line(&self.items[i])),
                None => String::new(),
            }
        } else if self.mode == Mode::Options {
            " Enter or space changes the highlighted option, left goes back, Esc closes."
                .to_string()
        } else {
            String::new()
        };
        let msg = if self.mode == Mode::Input {
            format!(
                "{}{}_",
                paint(&format!(" {}: ", self.prompt), &[BOLD]),
                self.input
            )
        } else if self.mode == Mode::Confirm {
            paint(
                &format!(" Discard {} unsaved change(s)? y/n", self.changes()),
                &[YELLOW, BOLD],
            )
        } else if !self.msg.is_empty() && self.msg_bad {
            paint(&format!(" {}", self.msg), &[RED])
        } else if !self.msg.is_empty() {
            paint(&format!(" {}", self.msg), &[GREEN])
        } else {
            String::new()
        };
        let keys = if self.screen == Screen::Settings {
            " Enter/space change  left back  w save  Tab repositories  ? help  q quit"
        } else if self.mode == Mode::Options {
            " up/down choose  Enter change  left back  Esc close"
        } else if self.mode == Mode::Search {
            " type to search  Enter keep  Esc clear"
        } else if self.mode == Mode::Input {
            " Enter accept  Esc cancel"
        } else {
            " space toggle  a sync  i ignore  r reset  f view  / search  o options  w save  Tab settings  ? help  q quit"
        };
        vec![
            paint(&clip_to(&detail, w), &[DIM]),
            msg,
            paint(&clip_to(keys, w), &[INVERT]),
        ]
    }
}

fn extra_text(it: &Item) -> String {
    let mut extra = note(it);
    if it.status == ST_WAITING {
        extra = "new, waiting for your decision".into();
    }
    let t = it.target();
    if !t.is_empty() && it.status != ST_ERROR && it.status != ST_WAITING {
        extra = t;
        let n = note(it);
        if !n.is_empty() && (it.status == ST_IGNORED || it.status == ST_FILTERED) {
            extra = n;
        }
    }
    extra
}

pub(crate) fn help_lines() -> Vec<String> {
    vec![
        paint(" Keys", &[BOLD]),
        String::new(),
        "  up/down, j/k      move          PgUp/PgDn, g/G   jump".into(),
        "  space             toggle this repository (sync or ignore)".into(),
        "  a / i / r         sync / ignore / back to the general rules".into(),
        "  A / I             sync / ignore every repository shown now".into(),
        "  f                 change the view: all, waiting, synced, skipped".into(),
        "  /                 search by name (Esc clears it)".into(),
        "  o or Enter        options for this repository: GitHub name, keep private, tags..."
            .into(),
        "  Tab               switch between Repositories and Settings".into(),
        "  w                 write the changes (a running gitsync applies them on its own)".into(),
        "  q                 quit (asks first if there are unsaved changes)".into(),
        String::new(),
        paint(
            "  [x] will sync   [ ] will not   [~] the general rules decide   * unsaved",
            &[DIM],
        ),
        String::new(),
        paint(" Press any key.", &[DIM]),
    ]
}

fn detail_line(it: &Item) -> String {
    let mut parts: Vec<String> = Vec::new();
    if !it.verdict.why.is_empty() {
        parts.push(it.verdict.why.clone());
    }
    parts.push(if it.repo.private { "private" } else { "public" }.into());
    if it.repo.fork {
        parts.push("fork".into());
    }
    if !it.repo.role.is_empty() {
        parts.push(format!("you: {}", it.repo.role));
    }
    if it.has && it.entry.last_ok > 0 {
        parts.push(format!("synced {}", ago(unix_now() - it.entry.last_ok)));
    }
    if !it.entry.last_error.is_empty() {
        parts.push(format!("error: {}", it.entry.last_error));
    }
    parts.join(" | ")
}
