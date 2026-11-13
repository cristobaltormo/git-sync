use std::borrow::Cow;
use std::fmt;

use serde::de::{self, Visitor};
use serde::{Deserialize, Deserializer};

use crate::config::Config;
use crate::error::Error;
use crate::httpx::{self, Client, Response};
use crate::logx;

use super::USER_AGENT;

pub(crate) struct Common {
    pub url: String,
    pub token: String,
    pub hook_url: String,
    pub secret: String,
}

impl Common {
    pub fn new(cfg: &Config) -> Common {
        Common {
            url: cfg.source_url().to_string(),
            token: cfg.source.token.clone(),
            hook_url: cfg.hook_url(),
            secret: cfg.listen.secret.clone(),
        }
    }

    pub fn client(&self, scheme: &str) -> Client {
        let auth = format!("{scheme} {}", self.token);
        Client::new(&[
            ("Authorization", &auth),
            ("Accept", "application/json"),
            ("User-Agent", USER_AGENT),
        ])
    }

    pub fn basic_header(&self, user: &str) -> String {
        let b = httpx::base64(format!("{user}:{}", self.token).as_bytes());
        logx::hide(&b);
        format!("Authorization: Basic {b}")
    }
}

pub(crate) fn rejected() -> Error {
    Error::api(401, "the source token was rejected")
}

pub(crate) fn check(r: Response) -> Result<Response, Error> {
    if r.ok() {
        Ok(r)
    } else {
        Err(httpx::fail(&r, ""))
    }
}

pub(crate) fn deleted(http: &Client, url: &str) -> bool {
    matches!(http.request("DELETE", url, None), Ok(r) if r.ok())
}

pub(crate) fn eq_fold(a: &str, b: &str) -> bool {
    if a.is_ascii() && b.is_ascii() {
        return a.eq_ignore_ascii_case(b);
    }
    a.chars()
        .flat_map(char::to_lowercase)
        .eq(b.chars().flat_map(char::to_lowercase))
}

pub(crate) fn cut(s: &str, sep: char) -> (&str, &str) {
    match s.split_once(sep) {
        Some((a, b)) => (a, b),
        None => (s, ""),
    }
}

fn escape(s: &str, keep: impl Fn(u8) -> bool, space_plus: bool) -> Cow<'_, str> {
    if s.bytes().all(&keep) {
        return Cow::Borrowed(s);
    }
    const HEX: &[u8; 16] = b"0123456789ABCDEF";
    let mut out = String::with_capacity(s.len() + 8);
    for b in s.bytes() {
        if keep(b) {
            out.push(b as char);
        } else if b == b' ' && space_plus {
            out.push('+');
        } else {
            out.push('%');
            out.push(HEX[(b >> 4) as usize] as char);
            out.push(HEX[(b & 15) as usize] as char);
        }
    }
    Cow::Owned(out)
}

fn unreserved(b: u8) -> bool {
    b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'~')
}

pub(crate) fn path_escape(s: &str) -> Cow<'_, str> {
    escape(
        s,
        |b| unreserved(b) || matches!(b, b'$' | b'&' | b'+' | b':' | b'=' | b'@'),
        false,
    )
}

pub(crate) fn query_escape(s: &str) -> Cow<'_, str> {
    escape(s, unreserved, true)
}

pub(crate) fn tolerant<T>(body: &[u8], f: impl Fn(&[u8]) -> Result<T, Error>) -> Result<T, Error> {
    match f(body) {
        Ok(v) => Ok(v),
        Err(e) => match repair(body) {
            Some(fixed) => f(&fixed),
            None => Err(e),
        },
    }
}

pub(crate) fn decode_with<T>(
    r: &Response,
    f: impl Fn(&[u8]) -> Result<T, Error>,
) -> Result<T, Error> {
    if r.body.is_empty() {
        return Err(Error::other("empty response"));
    }
    tolerant(&r.body, f)
}

pub(crate) fn decode<T: for<'a> Deserialize<'a>>(r: &Response) -> Result<T, Error> {
    decode_with(r, |b| Ok(serde_json::from_slice(b)?))
}

fn repair(body: &[u8]) -> Option<Vec<u8>> {
    let text = String::from_utf8_lossy(body);
    let b = text.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut changed = matches!(text, Cow::Owned(_));
    let mut i = 0;
    while i < b.len() {
        if b[i] != b'\\' {
            out.push(b[i]);
            i += 1;
            continue;
        }
        match escape_unit(b, i) {
            Some(0xD800..=0xDBFF) => {
                if let Some(0xDC00..=0xDFFF) = escape_unit(b, i + 6) {
                    out.extend_from_slice(&b[i..i + 12]);
                    i += 12;
                } else {
                    out.extend_from_slice(b"\\ufffd");
                    changed = true;
                    i += 6;
                }
            }
            Some(0xDC00..=0xDFFF) => {
                out.extend_from_slice(b"\\ufffd");
                changed = true;
                i += 6;
            }
            _ => {
                let end = (i + 2).min(b.len());
                out.extend_from_slice(&b[i..end]);
                i = end;
            }
        }
    }
    changed.then_some(out)
}

fn escape_unit(b: &[u8], i: usize) -> Option<u32> {
    let hex = b.get(i..i + 6)?;
    if hex[0] != b'\\' || hex[1] != b'u' {
        return None;
    }
    u32::from_str_radix(std::str::from_utf8(&hex[2..]).ok()?, 16).ok()
}

pub(crate) fn nullable<'de, D, T>(d: D) -> Result<T, D::Error>
where
    D: Deserializer<'de>,
    T: Default + Deserialize<'de>,
{
    Ok(Option::<T>::deserialize(d)?.unwrap_or_default())
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub(crate) struct Text<'a>(pub Cow<'a, str>);

impl Text<'_> {
    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    pub fn trimmed(&self) -> String {
        self.0.trim().to_string()
    }

    pub fn into_string(self) -> String {
        self.0.into_owned()
    }
}

impl<'de: 'a, 'a> Deserialize<'de> for Text<'a> {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Text<'a>, D::Error> {
        struct V;
        impl<'de> Visitor<'de> for V {
            type Value = Text<'de>;

            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("a string")
            }

            fn visit_borrowed_str<E: de::Error>(self, v: &'de str) -> Result<Text<'de>, E> {
                Ok(Text(Cow::Borrowed(v)))
            }

            fn visit_str<E: de::Error>(self, v: &str) -> Result<Text<'de>, E> {
                Ok(Text(Cow::Owned(v.to_string())))
            }

            fn visit_string<E: de::Error>(self, v: String) -> Result<Text<'de>, E> {
                Ok(Text(Cow::Owned(v)))
            }

            fn visit_unit<E: de::Error>(self) -> Result<Text<'de>, E> {
                Ok(Text::default())
            }

            fn visit_none<E: de::Error>(self) -> Result<Text<'de>, E> {
                Ok(Text::default())
            }
        }
        d.deserialize_any(V)
    }
}

#[cfg(test)]
pub(crate) mod fake {
    use hmac::{Hmac, KeyInit, Mac};
    use sha2::Sha256;

    use crate::config::Config;
    use crate::forge::{Change, Event, hex};
    use crate::httpx::Headers;
    use crate::logx;
    use crate::testutil::Reply;

    pub const TOKEN: &str = "src-token-123456";
    pub const SECRET: &str = "s3cret-value-123";

    pub fn config(kind: &str, url: &str) -> Config {
        logx::silence();
        let mut cfg = Config::default();
        cfg.source.kind = kind.into();
        cfg.source.url = url.into();
        cfg.source.token = TOKEN.into();
        cfg.listen.secret = SECRET.into();
        cfg
    }

    pub fn query<'a>(target: &'a str, key: &str) -> &'a str {
        let Some((_, q)) = target.split_once('?') else {
            return "";
        };
        q.split('&')
            .filter_map(|kv| kv.split_once('='))
            .find(|(k, _)| *k == key)
            .map(|(_, v)| v)
            .unwrap_or("")
    }

    pub fn mac(secret: &str, body: &str) -> String {
        let mut m = Hmac::<Sha256>::new_from_slice(secret.as_bytes()).unwrap();
        m.update(body.as_bytes());
        hex(&m.finalize().into_bytes())
    }

    pub fn headers(pairs: &[(&str, &str)]) -> Headers {
        let mut h = Headers::new();
        for (k, v) in pairs {
            h.insert(k, v);
        }
        h
    }

    pub fn js(v: &serde_json::Value) -> Reply {
        Reply::json(&v.to_string())
    }

    pub fn created(v: &serde_json::Value) -> Reply {
        Reply {
            status: 201,
            ..js(v)
        }
    }

    pub fn not_found() -> Reply {
        Reply::status(404).body("404 page not found")
    }

    pub fn must_event(
        got: Result<Option<Event>, crate::Error>,
        kind: Change,
        owner: &str,
        name: &str,
        id: i64,
    ) {
        let ev = got.expect("event").expect("an event");
        assert_eq!(
            ev,
            Event {
                kind,
                owner: owner.into(),
                name: name.into(),
                id
            }
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn escapes_like_go() {
        assert_eq!(path_escape("team/web"), "team%2Fweb");
        assert_eq!(path_escape("a b;c,d?e"), "a%20b%3Bc%2Cd%3Fe");
        assert_eq!(path_escape("{h1}"), "%7Bh1%7D");
        assert_eq!(path_escape("x$&+:=@y-_.~"), "x$&+:=@y-_.~");
        assert!(matches!(path_escape("plain"), Cow::Borrowed(_)));
        assert_eq!(
            query_escape("is:pr is:open user:a/b"),
            "is%3Apr+is%3Aopen+user%3Aa%2Fb"
        );
    }

    #[test]
    fn lone_surrogates_become_replacement_characters() {
        let src = br#"{"a":"x\ud83d y","b":"\ud83d\ude00","c":"\\ud83d","d":"\udc00"}"#;
        assert!(serde_json::from_slice::<serde_json::Value>(src).is_err());
        let v: serde_json::Value = tolerant(src, |b| Ok(serde_json::from_slice(b)?)).unwrap();
        assert_eq!(v["a"], "x\u{fffd} y");
        assert_eq!(v["b"], "\u{1f600}");
        assert_eq!(v["c"], "\\ud83d");
        assert_eq!(v["d"], "\u{fffd}");
        assert!(repair(br#"{"b":"\ud83d\ude00","c":"\\ud83d"}"#).is_none());
        let bad_utf8 = b"{\"a\":\"x\xff\"}";
        let v: serde_json::Value = tolerant(bad_utf8, |b| Ok(serde_json::from_slice(b)?)).unwrap();
        assert_eq!(v["a"], "x\u{fffd}");
        assert!(
            tolerant(b"{", |b| Ok(serde_json::from_slice::<serde_json::Value>(
                b
            )?))
            .is_err()
        );
    }

    #[test]
    fn case_folding() {
        assert!(eq_fold("Alice", "aLICE"));
        assert!(eq_fold("Ünï", "üNÏ"));
        assert!(!eq_fold("alice", "alicia"));
    }

    #[test]
    fn text_borrows_and_tolerates_null() {
        #[derive(Deserialize)]
        struct T<'a> {
            #[serde(borrow, default)]
            a: Text<'a>,
            #[serde(borrow, default)]
            b: Text<'a>,
            #[serde(borrow, default)]
            c: Text<'a>,
            #[serde(borrow, default)]
            d: Option<Text<'a>>,
            #[serde(default, deserialize_with = "nullable")]
            e: bool,
        }
        let src = br#"{"a":"plain","b":"esc\"aped","c":null,"d":null,"e":null}"#;
        let t: T<'_> = serde_json::from_slice(src).unwrap();
        assert!(matches!(t.a.0, Cow::Borrowed("plain")));
        assert_eq!(t.b.as_str(), "esc\"aped");
        assert!(t.c.is_empty());
        assert!(t.d.is_none());
        assert!(!t.e);
        assert!(serde_json::from_slice::<T<'_>>(br#"{"a":1}"#).is_err());
    }
}
