use std::io::IsTerminal;
use std::sync::LazyLock;

pub(crate) type Style = &'static str;

pub(crate) const RESET: Style = "\x1b[0m";
pub(crate) const BOLD: Style = "\x1b[1m";
pub(crate) const DIM: Style = "\x1b[2m";
pub(crate) const RED: Style = "\x1b[31m";
pub(crate) const GREEN: Style = "\x1b[32m";
pub(crate) const YELLOW: Style = "\x1b[33m";
pub(crate) const INVERT: Style = "\x1b[7m";

static COLOR_ON: LazyLock<bool> = LazyLock::new(detect_color);

fn detect_color() -> bool {
    if std::env::var_os("NO_COLOR").is_some_and(|v| !v.is_empty())
        || std::env::var_os("TERM").is_some_and(|v| v == "dumb")
    {
        return false;
    }
    if !std::io::stdout().is_terminal() {
        return false;
    }
    enable_vt()
}

#[cfg(not(windows))]
fn enable_vt() -> bool {
    true
}

#[cfg(windows)]
fn enable_vt() -> bool {
    use windows_sys::Win32::System::Console::{
        ENABLE_VIRTUAL_TERMINAL_PROCESSING, GetConsoleMode, GetStdHandle, STD_OUTPUT_HANDLE,
        SetConsoleMode,
    };
    unsafe {
        let h = GetStdHandle(STD_OUTPUT_HANDLE);
        let mut mode = 0u32;
        if GetConsoleMode(h, &mut mode) == 0 {
            return false;
        }
        SetConsoleMode(h, mode | ENABLE_VIRTUAL_TERMINAL_PROCESSING) != 0
    }
}

pub(crate) fn paint(s: &str, st: &[Style]) -> String {
    if !*COLOR_ON || st.is_empty() {
        return s.to_string();
    }
    let mut b = String::with_capacity(s.len() + 16);
    for x in st {
        b.push_str(x);
    }
    b.push_str(s);
    b.push_str(RESET);
    b
}

pub(crate) fn width(s: &str) -> usize {
    s.chars().count()
}

pub(crate) fn pad(s: &str, n: usize) -> String {
    let w = width(s);
    if w < n {
        let mut out = String::with_capacity(s.len() + n - w);
        out.push_str(s);
        out.extend(std::iter::repeat_n(' ', n - w));
        return out;
    }
    s.to_string()
}

pub(crate) fn clip(s: &str, n: usize) -> String {
    if n == 0 {
        return String::new();
    }
    let count = width(s);
    if count <= n {
        return s.to_string();
    }
    if n == 1 {
        return s.chars().take(1).collect();
    }
    let mut out: String = s.chars().take(n - 1).collect();
    out.push('~');
    out
}

pub(crate) fn stdin_is_terminal() -> bool {
    std::io::stdin().is_terminal()
}

pub(crate) fn stdout_is_terminal() -> bool {
    std::io::stdout().is_terminal()
}

pub(crate) struct Table {
    head: Vec<&'static str>,
    pub(crate) rows: Vec<Vec<String>>,
    sty: Vec<Vec<Style>>,
}

impl Table {
    pub(crate) fn new(head: &[&'static str]) -> Table {
        Table {
            head: head.to_vec(),
            rows: Vec::new(),
            sty: Vec::new(),
        }
    }

    pub(crate) fn add(&mut self, cells: Vec<String>, st: &[Style]) {
        self.rows.push(cells);
        self.sty.push(st.to_vec());
    }

    pub(crate) fn print(&self) {
        let mut cols: Vec<usize> = self.head.iter().map(|h| width(h)).collect();
        for r in &self.rows {
            for (i, c) in r.iter().enumerate() {
                cols[i] = cols[i].max(width(c));
            }
        }
        let line = |cells: &[&str], st: Style, per: &[Style]| {
            let mut b = String::from(" ");
            for (i, c) in cells.iter().enumerate() {
                let mut cell = if i == cells.len() - 1 {
                    c.to_string()
                } else {
                    pad(c, cols[i])
                };
                let s = match per.get(i) {
                    Some(p) if !p.is_empty() => p,
                    _ => st,
                };
                if !s.is_empty() {
                    cell = paint(&cell, &[s]);
                }
                b.push_str(&cell);
                b.push_str("  ");
            }
            println!("{}", b.trim_end_matches(' '));
        };
        line(&self.head, DIM, &[]);
        for (i, r) in self.rows.iter().enumerate() {
            let cells: Vec<&str> = r.iter().map(String::as_str).collect();
            line(&cells, "", &self.sty[i]);
        }
    }
}

pub(crate) fn heading(s: &str) {
    println!("{}", paint(s, &[BOLD]));
}

pub(crate) fn ago(sec: i64) -> String {
    match sec {
        s if s < 60 => "just now".into(),
        s if s < 3600 => format!("{} min ago", s / 60),
        s if s < 86400 => format!("{} h ago", s / 3600),
        s => format!("{} days ago", s / 86400),
    }
}

pub(crate) fn unix_now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

pub(crate) struct Local {
    pub(crate) year: i64,
    pub(crate) month: u32,
    pub(crate) day: u32,
    pub(crate) hour: u32,
    pub(crate) min: u32,
}

#[cfg(unix)]
#[allow(deprecated)]
pub(crate) fn local_time(t: i64) -> Local {
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    let tt = t as libc::time_t;
    let ok = unsafe { !libc::localtime_r(&tt, &mut tm).is_null() };
    if !ok {
        return utc(t);
    }
    Local {
        year: i64::from(tm.tm_year) + 1900,
        month: (tm.tm_mon + 1) as u32,
        day: tm.tm_mday as u32,
        hour: tm.tm_hour as u32,
        min: tm.tm_min as u32,
    }
}

#[cfg(windows)]
pub(crate) fn local_time(t: i64) -> Local {
    use windows_sys::Win32::Foundation::{FILETIME, SYSTEMTIME};
    use windows_sys::Win32::System::Time::{FileTimeToSystemTime, SystemTimeToTzSpecificLocalTime};
    let ticks = ((t + 11_644_473_600) as u64) * 10_000_000;
    let ft = FILETIME {
        dwLowDateTime: ticks as u32,
        dwHighDateTime: (ticks >> 32) as u32,
    };
    unsafe {
        let mut st: SYSTEMTIME = std::mem::zeroed();
        let mut local: SYSTEMTIME = std::mem::zeroed();
        if FileTimeToSystemTime(&ft, &mut st) == 0
            || SystemTimeToTzSpecificLocalTime(std::ptr::null(), &st, &mut local) == 0
        {
            return utc(t);
        }
        Local {
            year: i64::from(local.wYear),
            month: u32::from(local.wMonth),
            day: u32::from(local.wDay),
            hour: u32::from(local.wHour),
            min: u32::from(local.wMinute),
        }
    }
}

#[cfg(not(any(unix, windows)))]
pub(crate) fn local_time(t: i64) -> Local {
    utc(t)
}

fn utc(t: i64) -> Local {
    let days = t.div_euclid(86400);
    let rem = t.rem_euclid(86400);
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    Local {
        year: yoe + era * 400 + i64::from(m <= 2),
        month: m as u32,
        day: d as u32,
        hour: (rem / 3600) as u32,
        min: ((rem / 60) % 60) as u32,
    }
}

pub(crate) fn date_minutes(t: i64) -> String {
    let l = local_time(t);
    format!(
        "{:04}-{:02}-{:02} {:02}:{:02}",
        l.year, l.month, l.day, l.hour, l.min
    )
}

#[cfg(any(windows, test))]
pub(crate) fn iso_local(t: i64) -> String {
    let l = local_time(t);
    format!(
        "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}",
        l.year,
        l.month,
        l.day,
        l.hour,
        l.min,
        t.rem_euclid(60)
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pad_and_clip_count_characters() {
        assert_eq!(pad("ñu", 4), "ñu  ");
        assert_eq!(pad("long", 2), "long");
        assert_eq!(clip("añadido", 4), "aña~");
        assert_eq!(clip("abc", 1), "a");
        assert_eq!(clip("abc", 0), "");
        assert_eq!(clip("abc", 3), "abc");
    }

    #[test]
    fn ago_reads_like_go() {
        assert_eq!(ago(5), "just now");
        assert_eq!(ago(120), "2 min ago");
        assert_eq!(ago(7200), "2 h ago");
        assert_eq!(ago(3 * 86400), "3 days ago");
    }

    #[test]
    fn utc_dates() {
        let l = utc(1_700_000_000);
        assert_eq!(
            (l.year, l.month, l.day, l.hour, l.min),
            (2023, 11, 14, 22, 13)
        );
        assert!(iso_local(1_700_000_000).ends_with(":20"));
    }
}
