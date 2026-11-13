use std::io::Read;
use std::net::IpAddr;
use std::sync::{Arc, Condvar, Mutex, OnceLock};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use ureq::http::{self, HeaderMap, HeaderName, HeaderValue, Method, Uri};
#[cfg(any(all(test, unix), not(any(windows, target_os = "macos"))))]
use ureq::tls::PemItem;
use ureq::tls::{Certificate, RootCerts, TlsConfig};

use crate::error::Error;
use crate::logx;

const MAX_BODY: u64 = 32 << 20;

#[derive(Debug, Default, Clone)]
pub struct Headers(HeaderMap);

impl Headers {
    pub fn new() -> Headers {
        Headers(HeaderMap::new())
    }

    pub fn get(&self, name: &str) -> &str {
        self.value(name).unwrap_or("")
    }

    pub fn value(&self, name: &str) -> Option<&str> {
        self.0.get(name).and_then(|v| v.to_str().ok())
    }

    pub fn insert(&mut self, name: &str, value: &str) -> bool {
        self.insert_bytes(name.as_bytes(), value.as_bytes())
    }

    pub fn insert_bytes(&mut self, name: &[u8], value: &[u8]) -> bool {
        match (HeaderName::from_bytes(name), HeaderValue::from_bytes(value)) {
            (Ok(n), Ok(v)) => {
                self.0.append(n, v);
                true
            }
            _ => false,
        }
    }

    pub fn map(&self) -> &HeaderMap {
        &self.0
    }
}

#[derive(Debug)]
pub struct Response {
    pub status: u16,
    pub headers: Headers,
    pub body: Vec<u8>,
}

impl Response {
    pub fn ok(&self) -> bool {
        (200..300).contains(&self.status)
    }

    pub fn text(&self) -> String {
        let end = self.body.len().min(300);
        String::from_utf8_lossy(&self.body[..end]).into_owned()
    }

    pub fn decode<'a, T: Deserialize<'a>>(&'a self) -> Result<T, Error> {
        if self.body.is_empty() {
            return Err(Error::other("empty response"));
        }
        Ok(serde_json::from_slice(&self.body)?)
    }
}

pub fn fail(r: &Response, context: &str) -> Error {
    let text = r.text();
    let msg = if context.is_empty() {
        text
    } else {
        format!("{context}: {text}")
    };
    Error::api(r.status, msg)
}

pub fn base64(input: &[u8]) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(input.len().div_ceil(3) * 4);
    for chunk in input.chunks(3) {
        let b = [
            chunk[0],
            *chunk.get(1).unwrap_or(&0),
            *chunk.get(2).unwrap_or(&0),
        ];
        let n = (u32::from(b[0]) << 16) | (u32::from(b[1]) << 8) | u32::from(b[2]);
        for i in 0..4 {
            if i <= chunk.len() {
                out.push(TABLE[((n >> (18 - 6 * i)) & 63) as usize] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}

type Sleep = Box<dyn Fn(Duration) + Send + Sync>;

static CANCEL: (Mutex<bool>, Condvar) = (Mutex::new(false), Condvar::new());

pub fn cancel() {
    let mut c = CANCEL.0.lock().unwrap_or_else(|e| e.into_inner());
    *c = true;
    CANCEL.1.notify_all();
}

pub fn cancelled() -> bool {
    *CANCEL.0.lock().unwrap_or_else(|e| e.into_inner())
}

fn pause(d: Duration) {
    let c = CANCEL.0.lock().unwrap_or_else(|e| e.into_inner());
    let _ = CANCEL.1.wait_timeout_while(c, d, |c| !*c);
}

pub struct Client {
    headers: Vec<(HeaderName, HeaderValue)>,
    invalid: Option<String>,
    retries: u32,
    sleep: Sleep,
}

const MAX_REDIRECTS: usize = 10;

#[cfg(not(any(windows, target_os = "macos")))]
const CERT_FILES: &[&str] = &[
    "/etc/ssl/certs/ca-certificates.crt",
    "/etc/pki/tls/certs/ca-bundle.crt",
    "/etc/ssl/ca-bundle.pem",
    "/etc/pki/tls/cacert.pem",
    "/etc/pki/ca-trust/extracted/pem/tls-ca-bundle.pem",
    "/etc/ssl/cert.pem",
    "/usr/local/etc/ssl/cert.pem",
    "/usr/local/share/certs/ca-root-nss.crt",
    "/etc/openssl/certs/ca-certificates.crt",
];

#[cfg(not(any(windows, target_os = "macos")))]
const CERT_DIRS: &[&str] = &["/etc/ssl/certs", "/etc/pki/tls/certs"];

fn root_certs() -> RootCerts {
    static ROOTS: OnceLock<RootCerts> = OnceLock::new();
    ROOTS
        .get_or_init(|| {
            let certs = system_roots();
            if !certs.is_empty() {
                return RootCerts::from(certs);
            }
            no_system_roots()
        })
        .clone()
}

#[cfg(feature = "bundled-roots")]
fn no_system_roots() -> RootCerts {
    RootCerts::WebPki
}

#[cfg(not(feature = "bundled-roots"))]
fn no_system_roots() -> RootCerts {
    crate::warnf!("no CA certificates found on this system: set SSL_CERT_FILE or SSL_CERT_DIR");
    RootCerts::from(Vec::<Certificate<'static>>::new())
}

#[cfg(not(any(windows, target_os = "macos")))]
fn system_roots() -> Vec<Certificate<'static>> {
    load_roots(&crate::config::env_nonempty, CERT_FILES, CERT_DIRS)
}

#[cfg(any(windows, target_os = "macos"))]
fn system_roots() -> Vec<Certificate<'static>> {
    let mut certs: Vec<Certificate<'static>> = rustls_native_certs::load_native_certs()
        .certs
        .iter()
        .map(|c| Certificate::from_der(c.as_ref()).to_owned())
        .collect();
    certs.sort_by(|a, b| a.der().cmp(b.der()));
    certs.dedup_by(|a, b| a.der() == b.der());
    certs
}

#[cfg(any(all(test, unix), not(any(windows, target_os = "macos"))))]
pub(crate) fn load_roots(
    env: &dyn Fn(&str) -> Option<String>,
    files: &[&str],
    dirs: &[&str],
) -> Vec<Certificate<'static>> {
    let mut certs = Vec::new();
    let file = env("SSL_CERT_FILE");
    let files: Vec<&str> = match &file {
        Some(f) => vec![f.as_str()],
        None => files.to_vec(),
    };
    for f in files {
        if let Ok(data) = std::fs::read(f) {
            pem_certs(&data, &mut certs);
            break;
        }
    }
    match env("SSL_CERT_DIR") {
        Some(list) => list.split(':').for_each(|d| dir_certs(d, &mut certs)),
        None if certs.is_empty() => dirs.iter().for_each(|d| dir_certs(d, &mut certs)),
        None => {}
    }
    certs.sort_by(|a, b| a.der().cmp(b.der()));
    certs.dedup_by(|a, b| a.der() == b.der());
    certs
}

#[cfg(any(all(test, unix), not(any(windows, target_os = "macos"))))]
fn dir_certs(dir: &str, out: &mut Vec<Certificate<'static>>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for e in entries.flatten() {
        let path = e.path();
        if path.is_file()
            && let Ok(data) = std::fs::read(&path)
        {
            pem_certs(&data, out);
        }
    }
}

#[cfg(any(all(test, unix), not(any(windows, target_os = "macos"))))]
fn pem_certs(data: &[u8], out: &mut Vec<Certificate<'static>>) {
    for item in ureq::tls::parse_pem(data).flatten() {
        if let PemItem::Certificate(c) = item {
            out.push(c);
        }
    }
}

fn crypto_provider() -> Arc<rustls::crypto::CryptoProvider> {
    static PROVIDER: OnceLock<Arc<rustls::crypto::CryptoProvider>> = OnceLock::new();
    PROVIDER
        .get_or_init(|| Arc::new(rustls::crypto::ring::default_provider()))
        .clone()
}

fn agent_config() -> ureq::config::ConfigBuilder<ureq::typestate::AgentScope> {
    ureq::Agent::config_builder()
        .http_status_as_error(false)
        .user_agent(crate::forge::USER_AGENT)
        .tls_config(
            TlsConfig::builder()
                .root_certs(root_certs())
                .unversioned_rustls_crypto_provider(crypto_provider())
                .build(),
        )
        .timeout_global(Some(Duration::from_secs(60)))
        .timeout_connect(Some(Duration::from_secs(15)))
        .max_idle_connections(2)
        .max_idle_connections_per_host(1)
        .max_idle_age(Duration::from_secs(30))
        .input_buffer_size(16 * 1024)
        .output_buffer_size(16 * 1024)
}

pub fn agent() -> &'static ureq::Agent {
    static AGENT: OnceLock<ureq::Agent> = OnceLock::new();
    AGENT.get_or_init(|| {
        agent_config()
            .redirect_auth_headers(ureq::config::RedirectAuthHeaders::SameHost)
            .build()
            .new_agent()
    })
}

struct Net {
    direct: ureq::Agent,
    http: Option<ureq::Agent>,
    https: Option<ureq::Agent>,
    bypass: Vec<Bypass>,
}

fn net() -> &'static Net {
    static NET: OnceLock<Net> = OnceLock::new();
    NET.get_or_init(|| Net::from_env(&crate::config::env_nonempty))
}

fn env_any(env: &dyn Fn(&str) -> Option<String>, names: [&str; 2]) -> Option<String> {
    names.into_iter().find_map(env)
}

impl Net {
    fn from_env(env: &dyn Fn(&str) -> Option<String>) -> Net {
        let agent = |proxy: Option<ureq::Proxy>| {
            agent_config()
                .max_redirects(0)
                .proxy(proxy)
                .build()
                .new_agent()
        };
        let proxied = |names: [&str; 2]| {
            let url = env_any(env, names)?;
            let proxy = proxy_url(&url);
            match ureq::Proxy::new(&proxy) {
                Ok(p) => Some(agent(Some(p))),
                Err(e) => {
                    crate::warnf!("ignoring {}: {e}", names[0]);
                    None
                }
            }
        };
        Net {
            direct: agent(None),
            http: proxied(["HTTP_PROXY", "http_proxy"]),
            https: proxied(["HTTPS_PROXY", "https_proxy"]),
            bypass: env_any(env, ["NO_PROXY", "no_proxy"])
                .map(|v| parse_no_proxy(&v))
                .unwrap_or_default(),
        }
    }

    fn agent(&self, uri: &Uri) -> &ureq::Agent {
        let https = uri
            .scheme_str()
            .is_some_and(|s| s.eq_ignore_ascii_case("https"));
        let proxied = if https { &self.https } else { &self.http };
        match proxied {
            Some(a) => {
                let host = uri.host().unwrap_or("").to_ascii_lowercase();
                let port = uri.port_u16().unwrap_or(if https { 443 } else { 80 });
                if use_proxy(&self.bypass, &host, port) {
                    a
                } else {
                    &self.direct
                }
            }
            None => &self.direct,
        }
    }
}

fn proxy_url(raw: &str) -> String {
    match raw.split_once("://") {
        Some((scheme, _))
            if ["http", "https", "socks5", "socks5h"]
                .iter()
                .any(|s| scheme.eq_ignore_ascii_case(s)) =>
        {
            raw.to_string()
        }
        _ => format!("http://{raw}"),
    }
}

#[derive(Debug, PartialEq, Eq)]
enum Bypass {
    All,
    Net(IpAddr, u8),
    Ip(IpAddr, Option<u16>),
    Domain {
        suffix: String,
        exact: bool,
        port: Option<u16>,
    },
}

fn split_host_port(s: &str) -> Option<(&str, Option<u16>)> {
    let (host, port) = if let Some(rest) = s.strip_prefix('[') {
        let (h, tail) = rest.split_once(']')?;
        (h, tail.strip_prefix(':')?)
    } else {
        let (h, p) = s.split_once(':')?;
        if p.contains(':') {
            return None;
        }
        (h, p)
    };
    Some((host, Some(port.parse().ok()?)))
}

fn parse_cidr(s: &str) -> Option<(IpAddr, u8)> {
    let (ip, bits) = s.split_once('/')?;
    let ip: IpAddr = ip.parse().ok()?;
    let bits: u8 = bits.parse().ok()?;
    let max = if ip.is_ipv4() { 32 } else { 128 };
    (bits <= max).then_some((ip, bits))
}

fn parse_no_proxy(v: &str) -> Vec<Bypass> {
    let mut out = Vec::new();
    for p in v.split(',') {
        let p = p.trim().to_ascii_lowercase();
        if p.is_empty() {
            continue;
        }
        if p == "*" {
            return vec![Bypass::All];
        }
        if let Some((ip, bits)) = parse_cidr(&p) {
            out.push(Bypass::Net(ip, bits));
            continue;
        }
        let (host, port) = match split_host_port(&p) {
            Some(("", _)) => continue,
            Some(hp) => hp,
            None => (p.as_str(), None),
        };
        if let Ok(ip) = host.parse::<IpAddr>() {
            out.push(Bypass::Ip(ip, port));
            continue;
        }
        let host = host.strip_prefix('*').unwrap_or(host);
        if host.is_empty() {
            continue;
        }
        let (suffix, exact) = if host.starts_with('.') {
            (host.to_string(), false)
        } else {
            (format!(".{host}"), true)
        };
        out.push(Bypass::Domain {
            suffix,
            exact,
            port,
        });
    }
    out
}

fn in_net(ip: IpAddr, net: IpAddr, bits: u8) -> bool {
    match (ip, net) {
        (IpAddr::V4(a), IpAddr::V4(b)) => {
            let mask = u32::MAX.checked_shl(32 - u32::from(bits)).unwrap_or(0);
            u32::from(a) & mask == u32::from(b) & mask
        }
        (IpAddr::V6(a), IpAddr::V6(b)) => {
            let mask = u128::MAX.checked_shl(128 - u32::from(bits)).unwrap_or(0);
            u128::from(a) & mask == u128::from(b) & mask
        }
        _ => false,
    }
}

fn use_proxy(bypass: &[Bypass], host: &str, port: u16) -> bool {
    let host = host.trim_start_matches('[').trim_end_matches(']');
    if host == "localhost" {
        return false;
    }
    let ip = host.parse::<IpAddr>().ok();
    if ip.is_some_and(|ip| ip.is_loopback()) {
        return false;
    }
    let port_ok = |p: &Option<u16>| p.is_none_or(|p| p == port);
    !bypass.iter().any(|b| match b {
        Bypass::All => true,
        Bypass::Net(net, bits) => ip.is_some_and(|ip| in_net(ip, *net, *bits)),
        Bypass::Ip(want, p) => ip == Some(*want) && port_ok(p),
        Bypass::Domain {
            suffix,
            exact,
            port: p,
        } => (host.ends_with(suffix.as_str()) || (*exact && host == &suffix[1..])) && port_ok(p),
    })
}

fn resolve(base: &str, loc: &str) -> Option<String> {
    let has_scheme = loc.split_once(':').is_some_and(|(s, _)| {
        !s.is_empty()
            && s.bytes().next().is_some_and(|b| b.is_ascii_alphabetic())
            && s.bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'+' | b'-' | b'.'))
    });
    if has_scheme {
        return Some(loc.to_string());
    }
    let (scheme, rest) = base.split_once("://")?;
    if let Some(net_path) = loc.strip_prefix("//") {
        return Some(format!("{scheme}://{net_path}"));
    }
    let slash = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    let (authority, path) = rest.split_at(slash);
    let origin = format!("{scheme}://{authority}");
    if loc.starts_with('/') {
        return Some(format!("{origin}{loc}"));
    }
    let path = path.split(['?', '#']).next().unwrap_or("");
    if loc.starts_with('?') {
        return Some(format!("{origin}{path}{loc}"));
    }
    let dir = &path[..path.rfind('/').map_or(0, |i| i + 1)];
    let dir = if dir.is_empty() { "/" } else { dir };
    Some(format!("{origin}{dir}{loc}"))
}

fn host_of(url: &str) -> String {
    url.parse::<Uri>()
        .ok()
        .and_then(|u| u.host().map(str::to_ascii_lowercase))
        .unwrap_or_default()
}

fn same_site(dest: &str, initial: &str) -> bool {
    if dest == initial {
        return true;
    }
    if dest.contains([':', '%', '[']) {
        return false;
    }
    dest.strip_suffix(initial)
        .is_some_and(|head| head.ends_with('.'))
}

pub(crate) fn fix_surrogates(data: &mut Vec<u8>) {
    let hex4 = |at: usize, data: &[u8]| -> Option<u32> {
        let s = std::str::from_utf8(data.get(at..at + 4)?).ok()?;
        u32::from_str_radix(s, 16).ok()
    };
    let mut out: Option<Vec<u8>> = None;
    let mut copied = 0;
    let mut i = 0;
    while let Some(off) = data
        .get(i..)
        .and_then(|d| d.iter().position(|&b| b == b'\\'))
    {
        let at = i + off;
        i = at + 2;
        if data.get(at + 1) != Some(&b'u') {
            continue;
        }
        let Some(cp) = hex4(at + 2, data) else {
            continue;
        };
        i = at + 6;
        if (0xD800..0xDC00).contains(&cp)
            && data.get(at + 6) == Some(&b'\\')
            && data.get(at + 7) == Some(&b'u')
            && hex4(at + 8, data).is_some_and(|lo| (0xDC00..0xE000).contains(&lo))
        {
            i = at + 12;
            continue;
        }
        if !(0xD800..0xE000).contains(&cp) {
            continue;
        }
        let o = out.get_or_insert_with(|| Vec::with_capacity(data.len()));
        o.extend_from_slice(&data[copied..at]);
        o.extend_from_slice(b"\\ufffd");
        copied = at + 6;
    }
    if let Some(mut o) = out {
        o.extend_from_slice(&data[copied..]);
        *data = o;
    }
}

enum SendError {
    Fatal(String),
    Transport(String),
}

impl Client {
    pub fn new(headers: &[(&str, &str)]) -> Client {
        let mut invalid = None;
        let mut out = Vec::with_capacity(headers.len());
        for (k, v) in headers {
            match (
                HeaderName::from_bytes(k.as_bytes()),
                HeaderValue::from_str(v),
            ) {
                (Ok(name), Ok(mut value)) => {
                    if name == http::header::AUTHORIZATION || name.as_str().contains("token") {
                        value.set_sensitive(true);
                    }
                    out.push((name, value));
                }
                _ => {
                    invalid.get_or_insert_with(|| format!("invalid header field value for {k:?}"));
                }
            }
        }
        Client {
            headers: out,
            invalid,
            retries: 3,
            sleep: Box::new(pause),
        }
    }

    pub fn set_sleep(&mut self, f: impl Fn(Duration) + Send + Sync + 'static) {
        self.sleep = Box::new(f);
    }

    pub fn get(&self, url: &str) -> Result<Response, Error> {
        self.request("GET", url, None)
    }

    pub fn json<B: Serialize + ?Sized>(
        &self,
        method: &str,
        url: &str,
        body: &B,
    ) -> Result<Response, Error> {
        let payload = serde_json::to_vec(body)?;
        self.request(method, url, Some(&payload))
    }

    pub fn request(
        &self,
        method: &str,
        url: &str,
        payload: Option<&[u8]>,
    ) -> Result<Response, Error> {
        let method = Method::from_bytes(method.as_bytes())
            .map_err(|e| crate::err!("{method} {}: {e}", logx::clean(url)))?;
        if let Some(e) = &self.invalid {
            return Err(crate::err!("{method} {}: {e}", logx::clean(url)));
        }
        let mut delay = Duration::from_secs(1);
        let mut last_err = String::new();
        let mut attempt = 0;
        while attempt <= self.retries {
            let r = match self.send(&method, url, payload) {
                Ok(r) => r,
                Err(SendError::Fatal(e)) => {
                    return Err(crate::err!("{method} {}: {e}", logx::clean(url)));
                }
                Err(SendError::Transport(e)) => {
                    last_err = e;
                    if attempt < self.retries && !cancelled() {
                        (self.sleep)(delay);
                        if cancelled() {
                            break;
                        }
                        delay *= 3;
                        attempt += 1;
                        continue;
                    }
                    break;
                }
            };
            if r.status >= 500 && r.status != 501 && attempt < self.retries && !cancelled() {
                (self.sleep)(delay);
                if cancelled() {
                    return Ok(r);
                }
                delay *= 3;
                attempt += 1;
                continue;
            }
            if (r.status == 403 || r.status == 429) && attempt < self.retries && self.rate_wait(&r)
            {
                attempt += 1;
                continue;
            }
            return Ok(r);
        }
        Err(crate::err!("{method} {}: {}", logx::clean(url), last_err))
    }

    fn send(
        &self,
        method: &Method,
        url: &str,
        payload: Option<&[u8]>,
    ) -> Result<Response, SendError> {
        let initial = host_of(url);
        let mut method = method.clone();
        let mut url = url.to_string();
        let mut payload = payload;
        let mut credentials = true;
        let mut hops = 0;
        loop {
            let uri: Uri = url
                .parse()
                .map_err(|e: http::uri::InvalidUri| SendError::Fatal(e.to_string()))?;
            let agent = net().agent(&uri);
            let mut builder = http::Request::builder().method(method.clone()).uri(uri);
            for (k, v) in &self.headers {
                if credentials || !v.is_sensitive() {
                    builder = builder.header(k, v);
                }
            }
            let sent = match payload {
                Some(p)
                    if self
                        .headers
                        .iter()
                        .any(|(k, _)| k == http::header::CONTENT_TYPE) =>
                {
                    builder.body(p).map(|req| agent.run(req))
                }
                Some(p) => builder
                    .header(http::header::CONTENT_TYPE, "application/json")
                    .body(p)
                    .map(|req| agent.run(req)),
                None => builder.body(()).map(|req| agent.run(req)),
            };
            let resp = sent
                .map_err(|e| SendError::Fatal(e.to_string()))?
                .map_err(|e| SendError::Transport(e.to_string()))?;
            let (parts, body) = resp.into_parts();
            let status = parts.status.as_u16();
            let location = parts
                .headers
                .get(http::header::LOCATION)
                .and_then(|v| v.to_str().ok())
                .filter(|l| !l.is_empty());
            if let (301 | 302 | 303 | 307 | 308, Some(loc)) = (status, location) {
                if hops == MAX_REDIRECTS {
                    return Err(SendError::Transport(format!(
                        "stopped after {MAX_REDIRECTS} redirects"
                    )));
                }
                let next = resolve(&url, loc).ok_or_else(|| {
                    SendError::Transport("failed to parse Location header".into())
                })?;
                let _ = std::io::copy(&mut body.into_reader().take(64 << 10), &mut std::io::sink());
                hops += 1;
                if matches!(status, 301..=303) {
                    if method != Method::GET && method != Method::HEAD {
                        method = Method::GET;
                    }
                    payload = None;
                }
                credentials &= same_site(&host_of(&next), &initial);
                url = next;
                continue;
            }
            let mut data = Vec::new();
            let _ = body.into_reader().take(MAX_BODY).read_to_end(&mut data);
            fix_surrogates(&mut data);
            return Ok(Response {
                status,
                headers: Headers(parts.headers),
                body: data,
            });
        }
    }

    fn rate_wait(&self, r: &Response) -> bool {
        let mut wait: i64 = -1;
        let retry_after = r.headers.get("Retry-After");
        if !retry_after.is_empty() {
            if let Ok(n) = retry_after.trim().parse::<i64>() {
                wait = n;
            }
        } else if r.headers.get("X-RateLimit-Remaining") == "0"
            && let Ok(reset) = r.headers.get("X-RateLimit-Reset").trim().parse::<i64>()
        {
            let now = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|d| d.as_secs() as i64)
                .unwrap_or(0);
            wait = (reset - now).max(1);
        }
        if !(0..=120).contains(&wait) || cancelled() {
            return false;
        }
        crate::warnf!("rate limited, waiting {wait}s");
        (self.sleep)(Duration::from_secs(wait as u64 + 1));
        !cancelled()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::{Reply, TestServer};
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex};

    fn recorder(c: &mut Client) -> Arc<Mutex<Vec<Duration>>> {
        let slept = Arc::new(Mutex::new(Vec::new()));
        let s = slept.clone();
        c.set_sleep(move |d| s.lock().unwrap().push(d));
        slept
    }

    #[test]
    fn retries_server_errors_and_rate_limits() {
        logx::silence();
        let n = Arc::new(AtomicUsize::new(0));
        let calls = n.clone();
        let srv = TestServer::start(move |_| match calls.fetch_add(1, Ordering::SeqCst) + 1 {
            1 => Reply::status(502),
            2 => Reply::status(403).header("Retry-After", "3"),
            _ => Reply::json(r#"{"ok":true}"#),
        });
        let mut c = Client::new(&[]);
        let slept = recorder(&mut c);
        let r = c.get(&srv.url()).expect("should have recovered");
        assert!(r.ok());
        let slept = slept.lock().unwrap();
        assert_eq!(n.load(Ordering::SeqCst), 3);
        assert_eq!(*slept, vec![Duration::from_secs(1), Duration::from_secs(4)]);
    }

    #[test]
    fn gives_up_on_network_errors() {
        let mut c = Client::new(&[]);
        let slept = recorder(&mut c);
        assert!(c.get("http://127.0.0.1:1/x").is_err());
        assert_eq!(slept.lock().unwrap().len(), 3);
    }

    #[test]
    fn client_errors_are_returned_not_retried() {
        let n = Arc::new(AtomicUsize::new(0));
        let calls = n.clone();
        let srv = TestServer::start(move |_| {
            calls.fetch_add(1, Ordering::SeqCst);
            Reply::status(404)
        });
        let r = Client::new(&[]).get(&srv.url()).unwrap();
        assert_eq!(r.status, 404);
        assert_eq!(n.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn sends_headers_and_json() {
        let srv = TestServer::start(|req| {
            Reply::json(&format!(
                "{{\"auth\":\"{}\",\"ua\":\"{}\",\"ct\":\"{}\",\"body\":{}}}",
                req.header("authorization"),
                req.header("user-agent"),
                req.header("content-type"),
                String::from_utf8_lossy(&req.body)
            ))
        });
        let c = Client::new(&[("Authorization", "token abc"), ("User-Agent", "gitsync")]);
        let r = c
            .json(
                "PATCH",
                &format!("{}/x", srv.url()),
                &serde_json::json!({"a": 1}),
            )
            .unwrap();
        #[derive(Deserialize)]
        struct Echo<'a> {
            auth: &'a str,
            ua: &'a str,
            ct: &'a str,
            body: serde_json::Value,
        }
        let e: Echo<'_> = r.decode().unwrap();
        assert_eq!(e.auth, "token abc");
        assert_eq!(e.ua, "gitsync");
        assert_eq!(e.ct, "application/json");
        assert_eq!(e.body["a"], 1);
        let seen = &srv.requests()[0];
        assert_eq!(seen.authorization, "token abc");
        assert_eq!(seen.body, br#"{"a":1}"#);
        assert_eq!(srv.lines(), vec!["PATCH /x"]);
        assert_eq!(seen.target, "/x");
    }

    #[test]
    fn rate_limit_reset_in_the_far_future_is_not_waited() {
        let srv = TestServer::start(|_| {
            Reply::status(403)
                .header("X-RateLimit-Remaining", "0")
                .header("X-RateLimit-Reset", "99999999999")
        });
        let mut c = Client::new(&[]);
        let slept = recorder(&mut c);
        let r = c.get(&srv.url()).unwrap();
        assert_eq!(r.status, 403);
        assert!(slept.lock().unwrap().is_empty());
        assert!(
            fail(&r, "listing x")
                .to_string()
                .starts_with("HTTP 403: listing x: ")
        );
    }

    #[test]
    fn base64_matches_the_standard_encoding() {
        assert_eq!(base64(b""), "");
        assert_eq!(base64(b"f"), "Zg==");
        assert_eq!(base64(b"fo"), "Zm8=");
        assert_eq!(base64(b"foo"), "Zm9v");
        assert_eq!(base64(b"x-access-token:abc"), "eC1hY2Nlc3MtdG9rZW46YWJj");
    }

    #[test]
    fn redirects_follow_go_rules() {
        let target = TestServer::start(|_| Reply::json(r#"{"ok":true}"#));
        let same = target.url();
        let other = same.replace("127.0.0.1", "localhost");
        let hop = TestServer::start(move |req| match req.path() {
            "/keep" => Reply::status(307).header("Location", &format!("{same}/kept")),
            "/drop" => Reply::status(308).header("Location", &format!("{other}/dropped")),
            "/see" => Reply::status(303).header("Location", "/keep?x=1"),
            _ => Reply::status(302).header("Location", "loop"),
        });
        let c = Client::new(&[("Authorization", "token abc"), ("Private-Token", "t")]);
        let base = hop.url();
        assert!(
            c.json(
                "PATCH",
                &format!("{base}/keep"),
                &serde_json::json!({"a": 1})
            )
            .unwrap()
            .ok()
        );
        assert!(
            c.request("DELETE", &format!("{base}/drop"), None)
                .unwrap()
                .ok()
        );
        assert!(
            c.json("POST", &format!("{base}/see"), &serde_json::json!({"b": 2}))
                .unwrap()
                .ok()
        );
        let seen = target.requests();
        assert_eq!(seen[0].line(), "PATCH /kept");
        assert_eq!(seen[0].body, br#"{"a":1}"#);
        assert_eq!(seen[0].authorization, "token abc");
        assert_eq!(seen[1].line(), "DELETE /dropped");
        assert_eq!(seen[1].authorization, "");
        assert_eq!(seen[2].line(), "GET /kept");
        assert!(seen[2].body.is_empty());
        assert_eq!(seen[2].authorization, "token abc");
        assert_eq!(hop.lines()[2..4], ["POST /see", "GET /keep?x=1"]);
        let mut c = Client::new(&[]);
        let _ = recorder(&mut c);
        let err = c.get(&format!("{base}/a/loop")).unwrap_err();
        assert!(
            err.to_string().ends_with("stopped after 10 redirects"),
            "{err}"
        );
    }

    #[test]
    fn locations_resolve_like_urls() {
        let b = "https://h.example:8443/api/v1/repos?page=2";
        assert_eq!(resolve(b, "https://x.y/z").unwrap(), "https://x.y/z");
        assert_eq!(resolve(b, "//x.y/z").unwrap(), "https://x.y/z");
        assert_eq!(
            resolve(b, "/repositories/7").unwrap(),
            "https://h.example:8443/repositories/7"
        );
        assert_eq!(
            resolve(b, "?page=3").unwrap(),
            "https://h.example:8443/api/v1/repos?page=3"
        );
        assert_eq!(
            resolve(b, "other").unwrap(),
            "https://h.example:8443/api/v1/other"
        );
        assert!(same_site("api.github.com", "github.com"));
        assert!(same_site("github.com", "github.com"));
        assert!(!same_site("evilgithub.com", "github.com"));
        assert!(!same_site("github.com", "api.github.com"));
    }

    #[test]
    fn proxies_follow_go_environment_rules() {
        let env = |pairs: &'static [(&'static str, &'static str)]| {
            move |k: &str| {
                pairs
                    .iter()
                    .find(|(n, _)| *n == k)
                    .map(|(_, v)| v.to_string())
            }
        };
        let n = Net::from_env(&env(&[("HTTP_PROXY", "proxy.local:3128")]));
        assert!(
            n.http.is_some() && n.https.is_none(),
            "HTTP_PROXY alone must not proxy https"
        );
        let n = Net::from_env(&env(&[("ALL_PROXY", "http://proxy.local:3128")]));
        assert!(
            n.http.is_none() && n.https.is_none(),
            "ALL_PROXY is not read"
        );
        assert_eq!(proxy_url("proxy.local:3128"), "http://proxy.local:3128");
        assert_eq!(proxy_url("https://p:1"), "https://p:1");
        let b = parse_no_proxy(
            "example.com, .internal, *.corp, 10.0.0.0/8, 192.168.1.5, git.local:8443",
        );
        assert!(!use_proxy(&b, "localhost", 80));
        assert!(!use_proxy(&b, "127.0.0.1", 80));
        assert!(!use_proxy(&b, "::1", 443));
        assert!(!use_proxy(&b, "example.com", 443));
        assert!(!use_proxy(&b, "api.example.com", 443));
        assert!(use_proxy(&b, "internal", 443));
        assert!(!use_proxy(&b, "a.internal", 443));
        assert!(!use_proxy(&b, "x.corp", 443));
        assert!(!use_proxy(&b, "10.1.2.3", 443));
        assert!(!use_proxy(&b, "192.168.1.5", 443));
        assert!(use_proxy(&b, "192.168.1.6", 443));
        assert!(!use_proxy(&b, "git.local", 8443));
        assert!(use_proxy(&b, "git.local", 443));
        assert!(use_proxy(&b, "github.com", 443));
        assert!(!use_proxy(&parse_no_proxy("*"), "github.com", 443));
    }

    #[cfg(unix)]
    #[test]
    fn system_roots_are_loaded_before_the_bundled_ones() {
        let d = crate::testutil::TempDir::new();
        let pem = |body: &str| {
            format!(
                "-----BEGIN CERTIFICATE-----\n{}\n-----END CERTIFICATE-----\n",
                base64(body.as_bytes())
            )
        };
        let file = d.path().join("bundle.pem");
        std::fs::write(&file, pem("one") + &pem("two")).unwrap();
        let dir = d.path().join("certs");
        std::fs::create_dir(&dir).unwrap();
        std::fs::write(dir.join("a.pem"), pem("two")).unwrap();
        std::fs::write(dir.join("b.pem"), pem("three")).unwrap();
        let (file, dir) = (
            file.to_string_lossy().into_owned(),
            dir.to_string_lossy().into_owned(),
        );
        let none = |_: &str| None;
        assert_eq!(load_roots(&none, &[&file], &[]).len(), 2);
        assert_eq!(
            load_roots(&none, &["/nonexistent", &file], &[&dir]).len(),
            2
        );
        assert_eq!(load_roots(&none, &["/nonexistent"], &[&dir]).len(), 2);
        let both = |k: &str| match k {
            "SSL_CERT_FILE" => Some(file.clone()),
            "SSL_CERT_DIR" => Some(dir.clone()),
            _ => None,
        };
        assert_eq!(load_roots(&both, &["/nonexistent"], &[]).len(), 3);
        assert!(load_roots(&none, &["/nonexistent"], &["/nonexistent"]).is_empty());
    }

    #[test]
    fn lone_surrogates_decode_as_replacement_characters() {
        let fix = |s: &str| {
            let mut v = s.as_bytes().to_vec();
            fix_surrogates(&mut v);
            String::from_utf8(v).unwrap()
        };
        let u = |hex: &str| format!("{}{}{hex}", '\\', 'u');
        let doc = |s: String| format!(r#"{{"d":"{s}"}}"#);
        assert_eq!(
            fix(&doc(format!("a{}", u("d83d")))),
            doc(format!("a{}", u("fffd")))
        );
        assert_eq!(
            fix(&doc(format!("{}x", u("de00")))),
            doc(format!("{}x", u("fffd")))
        );
        let pair = doc(u("d83d") + &u("de00"));
        assert_eq!(fix(&pair), pair);
        let escaped = doc(format!("\\\\{}", &u("d83d")[1..]));
        assert_eq!(fix(&escaped), escaped);
        let short = doc(format!("{}\\\"{}", u("00e9"), u("d8")));
        assert_eq!(fix(&short), short);
        let srv = TestServer::start(move |_| {
            Reply::json(&format!(r#"{{"description":"cut {}"}}"#, u("d83d")))
        });
        let r = Client::new(&[]).get(&srv.url()).unwrap();
        let v: serde_json::Value = r.decode().unwrap();
        assert_eq!(v["description"], "cut \u{fffd}");
    }

    #[test]
    fn state_snapshots_tolerate_missing_fields() {
        let s: crate::github::Snap = serde_json::from_str(r#"{"description":"x"}"#).unwrap();
        assert!(!s.private && !s.archived);
        assert_eq!(s.description, "x");
    }

    #[test]
    fn invalid_credentials_fail_every_request_instead_of_being_dropped() {
        let srv = TestServer::start(|_| Reply::json("[]"));
        let c = Client::new(&[("Authorization", "token abc\r")]);
        let err = c.get(&srv.url()).unwrap_err().to_string();
        assert!(
            err.ends_with("invalid header field value for \"Authorization\""),
            "{err}"
        );
        assert!(!err.contains("abc"), "{err}");
        assert!(srv.requests().is_empty());
        let c = Client::new(&[("Authorization", "token caf\u{e9}")]);
        assert!(c.get(&srv.url()).is_err());
        assert!(srv.requests().is_empty());
    }

    #[test]
    fn empty_bodies_do_not_decode() {
        let r = Response {
            status: 200,
            headers: Headers::new(),
            body: Vec::new(),
        };
        let err = r.decode::<serde_json::Value>().unwrap_err();
        assert_eq!(err.to_string(), "empty response");
    }
}
