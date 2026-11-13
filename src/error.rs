use std::fmt;
use std::io;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Error {
    Config(String),
    Api { status: u16, msg: String },
    Signature,
    Blocked(String),
    Other(String),
}

impl Error {
    pub fn config(msg: impl Into<String>) -> Error {
        Error::Config(msg.into())
    }

    pub fn api(status: u16, msg: impl Into<String>) -> Error {
        Error::Api {
            status,
            msg: msg.into(),
        }
    }

    pub fn other(msg: impl Into<String>) -> Error {
        Error::Other(msg.into())
    }

    pub fn is_config(&self) -> bool {
        matches!(self, Error::Config(_))
    }

    pub fn status(&self) -> Option<u16> {
        match self {
            Error::Api { status, .. } => Some(*status),
            _ => None,
        }
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Config(m) | Error::Blocked(m) | Error::Other(m) => f.write_str(m),
            Error::Api { status, msg } => write!(f, "HTTP {status}: {msg}"),
            Error::Signature => f.write_str("bad signature"),
        }
    }
}

impl std::error::Error for Error {}

impl From<io::Error> for Error {
    fn from(e: io::Error) -> Error {
        Error::Other(io_message(&e))
    }
}

impl From<serde_json::Error> for Error {
    fn from(e: serde_json::Error) -> Error {
        Error::Other(e.to_string())
    }
}

pub fn io_message(e: &io::Error) -> String {
    let mut s = e.to_string();
    if let Some(i) = s.find(" (os error ") {
        s.truncate(i);
    }
    if let Some(first) = s.chars().next()
        && first.is_ascii_uppercase()
        && !s[1..].starts_with(|c: char| c.is_ascii_uppercase())
    {
        s[..1].make_ascii_lowercase();
    }
    s
}

#[macro_export]
macro_rules! config_err {
    ($($arg:tt)*) => { $crate::error::Error::Config(format!($($arg)*)) };
}

#[macro_export]
macro_rules! err {
    ($($arg:tt)*) => { $crate::error::Error::Other(format!($($arg)*)) };
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn io_messages_read_like_go() {
        let e = io::Error::from_raw_os_error(2);
        assert_eq!(io_message(&e), "no such file or directory");
    }

    #[test]
    fn api_errors_show_the_status() {
        assert_eq!(Error::api(422, "nope").to_string(), "HTTP 422: nope");
        assert_eq!(Error::api(404, "x").status(), Some(404));
        assert!(config_err!("a {}", 1).is_config());
        assert_eq!(err!("b {}", 2).to_string(), "b 2");
    }
}
