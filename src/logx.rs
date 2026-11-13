use std::borrow::Cow;
use std::fmt;
use std::io::Write;
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::{LazyLock, RwLock};
use std::time::Duration;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
#[repr(u8)]
pub enum Level {
    Debug = 0,
    Info = 1,
    Warn = 2,
    Error = 3,
    Off = 4,
}

static LEVEL: AtomicU8 = AtomicU8::new(Level::Info as u8);
static SECRETS: RwLock<Vec<String>> = RwLock::new(Vec::new());
static JOURNAL: LazyLock<bool> =
    LazyLock::new(|| std::env::var_os("JOURNAL_STREAM").is_some_and(|v| !v.is_empty()));

pub fn set_level(name: &str) {
    let lv = match name {
        "debug" => Level::Debug,
        "warn" => Level::Warn,
        "error" => Level::Error,
        "off" => Level::Off,
        _ => Level::Info,
    };
    LEVEL.store(lv as u8, Ordering::Relaxed);
}

pub fn silence() {
    LEVEL.store(Level::Off as u8, Ordering::Relaxed);
}

pub fn enabled(lv: Level) -> bool {
    lv as u8 >= LEVEL.load(Ordering::Relaxed)
}

pub fn hide(s: &str) {
    if s.len() < 6 {
        return;
    }
    let mut secrets = SECRETS.write().unwrap_or_else(|e| e.into_inner());
    if !secrets.iter().any(|h| h == s) {
        secrets.push(s.to_string());
    }
}

pub fn clean(s: &str) -> Cow<'_, str> {
    let secrets = SECRETS.read().unwrap_or_else(|e| e.into_inner());
    let mut out = Cow::Borrowed(s);
    for h in secrets.iter() {
        if out.contains(h.as_str()) {
            out = Cow::Owned(out.replace(h.as_str(), "***"));
        }
    }
    out
}

pub fn write(lv: Level, args: fmt::Arguments<'_>) {
    if !enabled(lv) {
        return;
    }
    let tag = match lv {
        Level::Debug => "debug",
        Level::Info => "info",
        Level::Warn => "warn",
        Level::Error | Level::Off => "error",
    };
    let raw = match args.as_str() {
        Some(s) => Cow::Borrowed(s),
        None => Cow::Owned(args.to_string()),
    };
    let msg = clean(&raw);
    let mut line = String::with_capacity(msg.len() + 32);
    if !*JOURNAL {
        line.push_str(&timestamp());
        line.push(' ');
    }
    line.push_str(&format!("{tag:<5} "));
    line.push_str(&msg);
    line.push('\n');
    let _ = std::io::stderr().lock().write_all(line.as_bytes());
}

#[macro_export]
macro_rules! debugf {
    ($($arg:tt)*) => {
        if $crate::logx::enabled($crate::logx::Level::Debug) {
            $crate::logx::write($crate::logx::Level::Debug, format_args!($($arg)*))
        }
    };
}

#[macro_export]
macro_rules! infof {
    ($($arg:tt)*) => {
        if $crate::logx::enabled($crate::logx::Level::Info) {
            $crate::logx::write($crate::logx::Level::Info, format_args!($($arg)*))
        }
    };
}

#[macro_export]
macro_rules! warnf {
    ($($arg:tt)*) => {
        if $crate::logx::enabled($crate::logx::Level::Warn) {
            $crate::logx::write($crate::logx::Level::Warn, format_args!($($arg)*))
        }
    };
}

#[macro_export]
macro_rules! errorf {
    ($($arg:tt)*) => {
        if $crate::logx::enabled($crate::logx::Level::Error) {
            $crate::logx::write($crate::logx::Level::Error, format_args!($($arg)*))
        }
    };
}

pub fn duration(d: Duration) -> String {
    let total = d.as_nanos();
    if total == 0 {
        return "0s".to_string();
    }
    if total < 1_000 {
        return format!("{total}ns");
    }
    if total < 1_000_000 {
        return format!("{}µs", trim_fraction(total, 1_000));
    }
    if total < 1_000_000_000 {
        return format!("{}ms", trim_fraction(total, 1_000_000));
    }
    let secs = d.as_secs();
    let (h, m, s) = (secs / 3600, (secs / 60) % 60, secs % 60);
    let frac = d.subsec_nanos();
    let mut out = String::new();
    if h > 0 {
        out.push_str(&format!("{h}h"));
    }
    if h > 0 || m > 0 {
        out.push_str(&format!("{m}m"));
    }
    out.push_str(&trim_fraction(
        u128::from(s) * 1_000_000_000 + u128::from(frac),
        1_000_000_000,
    ));
    out.push('s');
    out
}

fn trim_fraction(value: u128, unit: u128) -> String {
    let whole = value / unit;
    let rest = value % unit;
    if rest == 0 {
        return whole.to_string();
    }
    let width = unit.ilog10() as usize;
    let digits = format!("{rest:0width$}");
    format!("{whole}.{}", digits.trim_end_matches('0'))
}

#[cfg(unix)]
fn timestamp() -> String {
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    let now = unsafe { libc::time(std::ptr::null_mut()) };
    if unsafe { libc::localtime_r(&now, &mut tm) }.is_null() {
        return String::from("0000-00-00 00:00:00");
    }
    format!(
        "{:04}-{:02}-{:02} {:02}:{:02}:{:02}",
        tm.tm_year + 1900,
        tm.tm_mon + 1,
        tm.tm_mday,
        tm.tm_hour,
        tm.tm_min,
        tm.tm_sec
    )
}

#[cfg(windows)]
fn timestamp() -> String {
    let mut t: windows_sys::Win32::Foundation::SYSTEMTIME = unsafe { std::mem::zeroed() };
    unsafe { windows_sys::Win32::System::SystemInformation::GetLocalTime(&mut t) };
    format!(
        "{:04}-{:02}-{:02} {:02}:{:02}:{:02}",
        t.wYear, t.wMonth, t.wDay, t.wHour, t.wMinute, t.wSecond
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn secrets_are_hidden() {
        hide("supersecretvalue");
        hide("short");
        assert_eq!(clean("boom supersecretvalue boom"), "boom *** boom");
        assert_eq!(clean("short stays"), "short stays");
        assert!(matches!(clean("nothing here"), Cow::Borrowed(_)));
    }

    #[test]
    fn durations_print_like_go() {
        assert_eq!(duration(Duration::from_millis(1500)), "1.5s");
        assert_eq!(duration(Duration::from_secs(15)), "15s");
        assert_eq!(duration(Duration::from_millis(850)), "850ms");
        assert_eq!(duration(Duration::from_millis(61_250)), "1m1.25s");
        assert_eq!(duration(Duration::from_secs(3600)), "1h0m0s");
        assert_eq!(duration(Duration::ZERO), "0s");
    }

    #[test]
    fn timestamps_have_the_go_layout() {
        let t = timestamp();
        assert_eq!(t.len(), 19, "{t}");
        assert_eq!(&t[4..5], "-");
        assert_eq!(&t[10..11], " ");
    }
}
