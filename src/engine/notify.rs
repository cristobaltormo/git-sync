use std::collections::BTreeMap;
use std::time::{SystemTime, UNIX_EPOCH};

use super::Engine;
use crate::error::Error;
use crate::{err, httpx, logx};

pub const EVENT_PENDING: &str = "pending";
pub const EVENT_FAILING: &str = "failing";
pub const EVENT_PULL_REQUEST: &str = "pull_request";

const NOTIFY_STACK: usize = 1 << 20;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Event {
    pub kind: &'static str,
    pub repo: String,
    pub info: String,
}

impl Event {
    pub fn message(&self) -> String {
        match self.kind {
            EVENT_PENDING => format!(
                "New repository {} is waiting for your decision. Run `gitsync repos` to sync or ignore it.",
                self.repo
            ),
            EVENT_FAILING => format!("{} keeps failing to sync: {}", self.repo, self.info),
            EVENT_PULL_REQUEST => format!(
                "A pull request was opened on the GitHub mirror of {}: {}",
                self.repo, self.info
            ),
            _ => self.repo.clone(),
        }
    }
}

impl Engine {
    pub(super) fn notify(&self, ev: Event) {
        let cfg = self.config();
        let n = &cfg.notify;
        if n.url.is_empty() || self.dry || !n.events.iter().any(|k| k == ev.kind) {
            return;
        }
        let (url, format) = (n.url.clone(), n.format.clone());
        let spawned = std::thread::Builder::new()
            .name("notify".into())
            .stack_size(NOTIFY_STACK)
            .spawn(move || {
                if let Err(e) = post(&url, &format, &ev) {
                    warnf!("notification failed: {e}");
                }
            });
        if let Err(e) = spawned {
            warnf!("notification failed: {e}");
        }
    }
}

fn post(url: &str, format: &str, ev: &Event) -> Result<(), Error> {
    let msg = ev.message();
    let mut ctype = "application/json";
    let body: Vec<u8> = match format {
        "text" => {
            ctype = "text/plain; charset=utf-8";
            msg.into_bytes()
        }
        "slack" => serde_json::to_vec(&BTreeMap::from([("text", msg.as_str())]))?,
        "discord" => serde_json::to_vec(&BTreeMap::from([("content", msg.as_str())]))?,
        _ => {
            let now = rfc3339_utc(SystemTime::now());
            serde_json::to_vec(&BTreeMap::from([
                ("event", ev.kind),
                ("repo", ev.repo.as_str()),
                ("message", msg.as_str()),
                ("time", now.as_str()),
            ]))?
        }
    };
    let mut headers = vec![("Title", "gitsync")];
    if ctype != "application/json" {
        headers.push(("Content-Type", ctype));
    }
    let shown = redact(url);
    let hide = |m: String| m.replace(url, &shown).replace(&*logx::clean(url), &shown);
    let resp = httpx::Client::new(&headers)
        .request("POST", url, Some(&body))
        .map_err(|e| err!("{}", hide(e.to_string())))?;
    if resp.status >= 300 {
        let reason = ureq::http::StatusCode::from_u16(resp.status)
            .ok()
            .and_then(|s| s.canonical_reason())
            .unwrap_or("");
        return Err(err!("{shown} answered {} {reason}", resp.status));
    }
    Ok(())
}

pub(super) fn redact(url: &str) -> String {
    let (scheme, rest) = url.split_once("://").unwrap_or(("", url));
    let end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    let host = &rest[..end];
    let host = host.rsplit_once('@').map_or(host, |(_, h)| h);
    let tail = if end < rest.len() { "/***" } else { "" };
    if scheme.is_empty() {
        format!("{host}{tail}")
    } else {
        format!("{scheme}://{host}{tail}")
    }
}

pub fn send_test(url: &str, format: &str) -> Result<(), Error> {
    post(
        url,
        format,
        &Event {
            kind: EVENT_PENDING,
            repo: "my-org/example".into(),
            info: String::new(),
        },
    )
}

pub(super) fn rfc3339_utc(t: SystemTime) -> String {
    let secs = t
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    let days = secs.div_euclid(86400);
    let rem = secs.rem_euclid(86400);
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!(
        "{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}Z",
        rem / 3600,
        (rem / 60) % 60,
        rem % 60
    )
}
