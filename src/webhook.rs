use std::io::{self, Read, Write};
use std::net::{Shutdown, SocketAddr, TcpListener, TcpStream, ToSocketAddrs};
use std::panic::{self, AssertUnwindSafe};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use crate::config;
use crate::config_err;
use crate::error::Error;
use crate::forge::{Event, Provider};
use crate::httpx::Headers;

pub const MAX_BODY: usize = 10 << 20;
const MAX_HEADER: usize = 64 << 10;
const MAX_FRAMING: usize = 64 << 10;
const HEADER_TIMEOUT: Duration = Duration::from_secs(10);
const READ_TIMEOUT: Duration = Duration::from_secs(30);
const WRITE_TIMEOUT: Duration = Duration::from_secs(30);
const DRAIN_TIMEOUT: Duration = Duration::from_secs(2);
const MAX_CONNECTIONS: usize = 256;

#[derive(Debug)]
pub struct Request {
    pub method: String,
    pub target: String,
    pub headers: Headers,
    pub body: Vec<u8>,
}

impl Request {
    pub fn path(&self) -> &str {
        self.target.split('?').next().unwrap_or("")
    }

    pub fn header(&self, name: &str) -> &str {
        self.headers.get(name)
    }
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum ReadError {
    Closed,
    Timeout,
    Bad,
    HeaderTooLarge,
    BodyTooLarge,
}

struct Conn<'a> {
    stream: &'a TcpStream,
    buf: Vec<u8>,
    pos: usize,
    header_deadline: Instant,
    deadline: Instant,
    in_header: bool,
    received: usize,
    budget: usize,
}

impl Conn<'_> {
    fn fill(&mut self) -> Result<(), ReadError> {
        if self.received >= self.budget {
            return Err(if self.in_header {
                ReadError::HeaderTooLarge
            } else {
                ReadError::BodyTooLarge
            });
        }
        let deadline = if self.in_header {
            self.header_deadline.min(self.deadline)
        } else {
            self.deadline
        };
        let left = deadline.saturating_duration_since(Instant::now());
        if left.is_zero() {
            return Err(ReadError::Timeout);
        }
        self.stream
            .set_read_timeout(Some(left))
            .map_err(|_| ReadError::Closed)?;
        let mut chunk = [0u8; 8192];
        let want = chunk.len().min(self.budget - self.received);
        loop {
            return match (&mut &*self.stream).read(&mut chunk[..want]) {
                Ok(0) => Err(ReadError::Closed),
                Ok(n) => {
                    self.received += n;
                    self.buf.extend_from_slice(&chunk[..n]);
                    Ok(())
                }
                Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
                Err(e)
                    if matches!(
                        e.kind(),
                        io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut
                    ) =>
                {
                    Err(ReadError::Timeout)
                }
                Err(_) => Err(ReadError::Closed),
            };
        }
    }

    fn line(&mut self, limit: usize) -> Result<Vec<u8>, ReadError> {
        let mut scanned = 0;
        loop {
            let from = self.pos + scanned;
            if let Some(i) = self.buf[from..].iter().position(|&b| b == b'\n') {
                let end = from + i;
                let mut line = &self.buf[self.pos..end];
                if line.last() == Some(&b'\r') {
                    line = &line[..line.len() - 1];
                }
                let line = line.to_vec();
                self.pos = end + 1;
                return Ok(line);
            }
            scanned = self.buf.len() - self.pos;
            if scanned > limit {
                return Err(if self.in_header {
                    ReadError::HeaderTooLarge
                } else {
                    ReadError::Bad
                });
            }
            self.fill()?;
        }
    }

    fn take(&mut self, n: usize, out: &mut Vec<u8>) -> Result<(), ReadError> {
        while self.buf.len() - self.pos < n {
            self.fill()?;
        }
        out.extend_from_slice(&self.buf[self.pos..self.pos + n]);
        self.pos += n;
        if self.pos > 1 << 20 {
            self.buf.drain(..self.pos);
            self.pos = 0;
        }
        Ok(())
    }
}

#[cfg(test)]
pub(crate) fn read_request(stream: &TcpStream, max_body: usize) -> Result<Request, ReadError> {
    read_request_until(stream, max_body, Instant::now() + READ_TIMEOUT)
}

fn read_request_until(
    stream: &TcpStream,
    max_body: usize,
    deadline: Instant,
) -> Result<Request, ReadError> {
    let mut c = Conn {
        stream,
        buf: Vec::with_capacity(4096),
        pos: 0,
        header_deadline: Instant::now() + HEADER_TIMEOUT,
        deadline,
        in_header: true,
        received: 0,
        budget: MAX_HEADER
            .saturating_add(max_body)
            .saturating_add(MAX_FRAMING),
    };
    let mut first = c.line(MAX_HEADER)?;
    while first.is_empty() {
        if c.pos > MAX_HEADER {
            return Err(ReadError::HeaderTooLarge);
        }
        first = c.line(MAX_HEADER)?;
    }
    let first = String::from_utf8(first).map_err(|_| ReadError::Bad)?;
    let mut parts = first.split(' ');
    let (Some(method), Some(target), Some(version), None) =
        (parts.next(), parts.next(), parts.next(), parts.next())
    else {
        return Err(ReadError::Bad);
    };
    if !version.starts_with("HTTP/1.") || method.is_empty() || !target.starts_with('/') {
        return Err(ReadError::Bad);
    }
    let (method, target) = (method.to_string(), target.to_string());
    let mut headers = Headers::new();
    loop {
        if c.pos > MAX_HEADER {
            return Err(ReadError::HeaderTooLarge);
        }
        let line = c.line(MAX_HEADER)?;
        if line.is_empty() {
            break;
        }
        if matches!(line[0], b' ' | b'\t') {
            return Err(ReadError::Bad);
        }
        let colon = line.iter().position(|&b| b == b':').ok_or(ReadError::Bad)?;
        let (name, value) = (&line[..colon], line[colon + 1..].trim_ascii());
        if name.is_empty()
            || name.ends_with(b" ")
            || name.ends_with(b"\t")
            || !headers.insert_bytes(name, value)
        {
            return Err(ReadError::Bad);
        }
    }
    c.in_header = false;
    let chunked = match headers.map().get("transfer-encoding").map(|v| v.as_bytes()) {
        None => false,
        Some(te) if te.eq_ignore_ascii_case(b"chunked") => true,
        Some(_) => return Err(ReadError::Bad),
    };
    let mut lengths = headers.map().get_all("content-length").iter();
    let length = match (lengths.next(), lengths.next()) {
        (None, _) => None,
        (Some(v), None) => {
            let v = v.as_bytes();
            if v.is_empty() || !v.iter().all(u8::is_ascii_digit) {
                return Err(ReadError::Bad);
            }
            let v = std::str::from_utf8(v).map_err(|_| ReadError::Bad)?;
            Some(v.parse::<u64>().map_err(|_| ReadError::BodyTooLarge)?)
        }
        _ => return Err(ReadError::Bad),
    };
    if chunked && length.is_some() {
        return Err(ReadError::Bad);
    }
    if length.is_some_and(|n| n > max_body as u64) {
        return Err(ReadError::BodyTooLarge);
    }
    if headers.get("Expect").eq_ignore_ascii_case("100-continue") {
        let _ = (&mut &*stream).write_all(b"HTTP/1.1 100 Continue\r\n\r\n");
    }
    let mut body = Vec::new();
    if chunked {
        loop {
            let line = c.line(1024)?;
            let size = line
                .split(|&b| b == b';')
                .next()
                .unwrap_or(&[])
                .trim_ascii();
            if size.is_empty() || size.len() > 16 || !size.iter().all(u8::is_ascii_hexdigit) {
                return Err(ReadError::Bad);
            }
            let size = std::str::from_utf8(size)
                .ok()
                .and_then(|s| u64::from_str_radix(s, 16).ok())
                .ok_or(ReadError::Bad)?;
            if size == 0 {
                let start = c.pos;
                while !c.line(MAX_HEADER)?.is_empty() {
                    if c.pos - start > MAX_HEADER {
                        return Err(ReadError::HeaderTooLarge);
                    }
                }
                break;
            }
            let room = max_body.saturating_sub(body.len());
            let size = match usize::try_from(size) {
                Ok(n) if n <= room => n,
                _ => return Err(ReadError::BodyTooLarge),
            };
            c.take(size, &mut body)?;
            if !c.line(2)?.is_empty() {
                return Err(ReadError::Bad);
            }
        }
    } else if let Some(n) = length {
        let n = n as usize;
        body.reserve_exact(n);
        c.take(n, &mut body)?;
    }
    Ok(Request {
        method,
        target,
        headers,
        body,
    })
}

fn reason(code: u16) -> &'static str {
    match code {
        200 => "OK",
        202 => "Accepted",
        400 => "Bad Request",
        403 => "Forbidden",
        404 => "Not Found",
        408 => "Request Timeout",
        413 => "Request Entity Too Large",
        431 => "Request Header Fields Too Large",
        500 => "Internal Server Error",
        503 => "Service Unavailable",
        _ => "Unknown",
    }
}

pub(crate) fn write_response(
    stream: &TcpStream,
    code: u16,
    headers: &[(&str, &str)],
    body: &[u8],
) -> io::Result<()> {
    let mut out = format!("HTTP/1.1 {code} {}\r\n", reason(code));
    for (k, v) in headers {
        out.push_str(k);
        out.push_str(": ");
        out.push_str(v);
        out.push_str("\r\n");
    }
    out.push_str(&format!(
        "Content-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    ));
    let mut bytes = out.into_bytes();
    bytes.extend_from_slice(body);
    let mut s = stream;
    s.write_all(&bytes)?;
    s.flush()
}

const TEXT: (&str, &str) = ("Content-Type", "text/plain; charset=utf-8");
const NOSNIFF: (&str, &str) = ("X-Content-Type-Options", "nosniff");

fn reply_error(stream: &TcpStream, code: u16, msg: &str, deadline: Instant) {
    let body = format!("{msg}\n");
    if write_response(stream, code, &[TEXT, NOSNIFF], body.as_bytes()).is_ok() {
        drain(stream, deadline);
    }
}

fn drain(stream: &TcpStream, deadline: Instant) {
    let _ = stream.shutdown(Shutdown::Write);
    let deadline = deadline.min(Instant::now() + DRAIN_TIMEOUT);
    let mut buf = [0u8; 8192];
    let mut left: usize = 1 << 20;
    while left > 0 {
        let wait = deadline.saturating_duration_since(Instant::now());
        if wait.is_zero() || stream.set_read_timeout(Some(wait)).is_err() {
            break;
        }
        match (&mut &*stream).read(&mut buf) {
            Ok(0) | Err(_) => break,
            Ok(n) => left = left.saturating_sub(n),
        }
    }
}

type SecretFn = Box<dyn Fn() -> String + Send + Sync>;
type DeliverFn = Box<dyn Fn(Event) + Send + Sync>;

pub struct Handler {
    src: Arc<dyn Provider>,
    secret: SecretFn,
    deliver: DeliverFn,
}

impl Handler {
    pub fn new(
        src: Arc<dyn Provider>,
        secret: impl Fn() -> String + Send + Sync + 'static,
        deliver: impl Fn(Event) + Send + Sync + 'static,
    ) -> Handler {
        Handler {
            src,
            secret: Box::new(secret),
            deliver: Box::new(deliver),
        }
    }

    pub fn serve(&self, stream: &TcpStream, remote: &SocketAddr) {
        let deadline = Instant::now() + READ_TIMEOUT;
        let _ = stream.set_write_timeout(Some(WRITE_TIMEOUT));
        let req = match read_request_until(stream, MAX_BODY, deadline) {
            Ok(r) => r,
            Err(ReadError::BodyTooLarge) => {
                return reply_error(stream, 413, "body too large", deadline);
            }
            Err(ReadError::HeaderTooLarge) => {
                return reply_error(stream, 431, "request header fields too large", deadline);
            }
            Err(ReadError::Bad) => return reply_error(stream, 400, "bad request", deadline),
            Err(ReadError::Timeout | ReadError::Closed) => return,
        };
        if req.path() == "/health" {
            let _ = write_response(stream, 200, &[TEXT], b"ok\n");
            return;
        }
        if req.method != "POST" {
            reply_error(stream, 404, "404 page not found", deadline);
            return;
        }
        match self
            .src
            .parse_webhook(&req.headers, &req.body, &(self.secret)())
        {
            Err(Error::Signature) => {
                crate::warnf!("webhook rejected: bad or missing signature from {remote}");
                reply_error(stream, 403, "bad signature", deadline);
            }
            Err(_) => reply_error(stream, 400, "bad request", deadline),
            Ok(ev) => {
                if let Some(ev) = ev {
                    crate::debugf!("webhook for {}/{}", ev.owner, ev.name);
                    (self.deliver)(ev);
                }
                let _ = write_response(stream, 202, &[TEXT], b"ok\n");
            }
        }
    }
}

pub struct Server {
    addr: SocketAddr,
    stop: Arc<AtomicBool>,
    live: Arc<Live>,
    thread: Option<JoinHandle<()>>,
}

const CLOSE_WAIT: Duration = Duration::from_secs(5);

impl Server {
    pub fn local_addr(&self) -> SocketAddr {
        self.addr
    }

    pub fn close(self) {
        self.close_within(CLOSE_WAIT);
    }

    pub fn close_within(mut self, wait: Duration) -> bool {
        self.shutdown();
        let n = self.live.n.lock().unwrap_or_else(|e| e.into_inner());
        let (n, _) = self
            .live
            .idle
            .wait_timeout_while(n, wait, |n| *n > 0)
            .unwrap_or_else(|e| e.into_inner());
        *n == 0
    }

    fn shutdown(&mut self) {
        let Some(t) = self.thread.take() else { return };
        self.stop.store(true, Ordering::SeqCst);
        let mut wake = self.addr;
        if wake.ip().is_unspecified() {
            wake.set_ip(match wake {
                SocketAddr::V4(_) => [127, 0, 0, 1].into(),
                SocketAddr::V6(_) => std::net::Ipv6Addr::LOCALHOST.into(),
            });
        }
        if TcpStream::connect_timeout(&wake, Duration::from_secs(1)).is_ok() {
            let _ = t.join();
        }
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        self.shutdown();
    }
}

fn dual_stack() -> bool {
    if cfg!(target_os = "linux") {
        return std::fs::read_to_string("/proc/sys/net/ipv6/bindv6only")
            .map_or(true, |v| v.trim() == "0");
    }
    cfg!(any(target_os = "macos", target_os = "freebsd"))
}

fn bind_addrs(host: &str, port: i64) -> io::Result<Vec<SocketAddr>> {
    if host.is_empty() {
        let mut out = Vec::with_capacity(2);
        if dual_stack() {
            out.extend(config::host_port("::", port).to_socket_addrs()?);
        }
        out.extend(config::host_port("0.0.0.0", port).to_socket_addrs()?);
        return Ok(out);
    }
    let mut all: Vec<SocketAddr> = config::host_port(host, port).to_socket_addrs()?.collect();
    all.sort_by_key(|a| !a.is_ipv4());
    Ok(all)
}

pub fn listen(host: &str, port: i64, handler: Handler) -> Result<Server, Error> {
    let addr = config::host_port(host, port);
    let fail =
        |e: &io::Error| config_err!("cannot listen on {addr}: {}", crate::error::io_message(e));
    let candidates = bind_addrs(host, port).map_err(|e| fail(&e))?;
    let mut last = None;
    for a in &candidates {
        match TcpListener::bind(a) {
            Ok(ln) => return serve(ln, handler),
            Err(e) => {
                last = Some(e);
                if !host.is_empty() {
                    break;
                }
            }
        }
    }
    Err(match last {
        Some(e) => fail(&e),
        None => config_err!("cannot listen on {addr}: no address"),
    })
}

pub fn serve(ln: TcpListener, handler: Handler) -> Result<Server, Error> {
    let addr = ln.local_addr()?;
    let stop = Arc::new(AtomicBool::new(false));
    let flag = stop.clone();
    let live = Arc::new(Live::default());
    let active = live.clone();
    let handler = Arc::new(handler);
    let thread = thread::Builder::new()
        .name("webhook".into())
        .stack_size(64 * 1024)
        .spawn(move || accept_loop(ln, handler, flag, active))?;
    Ok(Server {
        addr,
        stop,
        live,
        thread: Some(thread),
    })
}

#[derive(Default)]
struct Live {
    n: Mutex<usize>,
    idle: Condvar,
}

impl Live {
    #[cfg(test)]
    fn count(&self) -> usize {
        *self.n.lock().unwrap_or_else(|e| e.into_inner())
    }
}

struct Slot(Arc<Live>);

impl Slot {
    fn take(live: &Arc<Live>, max: usize) -> Option<Slot> {
        let mut n = live.n.lock().unwrap_or_else(|e| e.into_inner());
        if *n >= max {
            return None;
        }
        *n += 1;
        Some(Slot(live.clone()))
    }
}

impl Drop for Slot {
    fn drop(&mut self) {
        let mut n = self.0.n.lock().unwrap_or_else(|e| e.into_inner());
        *n -= 1;
        if *n == 0 {
            self.0.idle.notify_all();
        }
    }
}

fn busy(stream: TcpStream) {
    let _ = stream.set_write_timeout(Some(Duration::from_secs(1)));
    let _ = write_response(&stream, 503, &[TEXT, NOSNIFF], b"busy, try again later\n");
    let _ = stream.shutdown(Shutdown::Write);
}

fn accept_loop(ln: TcpListener, handler: Arc<Handler>, stop: Arc<AtomicBool>, active: Arc<Live>) {
    for conn in ln.incoming() {
        if stop.load(Ordering::SeqCst) {
            return;
        }
        let stream = match conn {
            Ok(s) => s,
            Err(_) => {
                thread::sleep(Duration::from_millis(100));
                continue;
            }
        };
        let Some(slot) = Slot::take(&active, MAX_CONNECTIONS) else {
            busy(stream);
            continue;
        };
        let remote = stream
            .peer_addr()
            .unwrap_or_else(|_| SocketAddr::from(([0, 0, 0, 0], 0)));
        let h = handler.clone();
        let _ = thread::Builder::new()
            .name("webhook-conn".into())
            .stack_size(1 << 20)
            .spawn(move || {
                let _slot = slot;
                let served = panic::catch_unwind(AssertUnwindSafe(|| h.serve(&stream, &remote)));
                if served.is_err() {
                    let _ = write_response(&stream, 500, &[TEXT, NOSNIFF], b"internal error\n");
                }
            });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::forge::{Change, Hooks, Identity, Repo, Source, valid_mac};
    use crate::logx;
    use hmac::{Hmac, KeyInit, Mac};
    use sha2::Sha256;
    use std::sync::Mutex;

    struct Fake;

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
        fn clone_url(&self, _: &Repo) -> String {
            String::new()
        }
        fn git_header(&self) -> Result<String, Error> {
            Ok(String::new())
        }
        fn parse_webhook(
            &self,
            h: &Headers,
            body: &[u8],
            secret: &str,
        ) -> Result<Option<Event>, Error> {
            if !h.get("X-Panic").is_empty() {
                panic!("provider bug");
            }
            if !valid_mac(secret, body, h.get("X-Gitea-Signature"), "") {
                return Err(Error::Signature);
            }
            #[derive(serde::Deserialize)]
            struct Payload<'a> {
                #[serde(borrow)]
                repository: RepoRef<'a>,
            }
            #[derive(serde::Deserialize)]
            struct RepoRef<'a> {
                id: i64,
                full_name: &'a str,
            }
            let p: Payload<'_> = serde_json::from_slice(body)?;
            if h.get("X-Gitea-Event") != "push" {
                return Ok(None);
            }
            let (owner, name) = p.repository.full_name.split_once('/').unwrap_or_default();
            Ok(Some(Event {
                kind: Change::Changed,
                owner: owner.into(),
                name: name.into(),
                id: p.repository.id,
            }))
        }
    }

    fn sign(secret: &str, body: &str) -> String {
        let mut m = Hmac::<Sha256>::new_from_slice(secret.as_bytes()).unwrap();
        m.update(body.as_bytes());
        crate::forge::hex(&m.finalize().into_bytes())
    }

    fn start() -> (Server, Arc<Mutex<Vec<Event>>>) {
        logx::silence();
        let got = Arc::new(Mutex::new(Vec::new()));
        let sink = got.clone();
        let h = Handler::new(
            Arc::new(Fake),
            || "s3cret".to_string(),
            move |e| sink.lock().unwrap().push(e),
        );
        let srv = listen("127.0.0.1", 0, h).unwrap();
        (srv, got)
    }

    fn raw(addr: SocketAddr, req: &[u8]) -> String {
        let mut s = TcpStream::connect(addr).unwrap();
        s.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
        s.write_all(req).unwrap();
        let mut out = String::new();
        let _ = s.read_to_string(&mut out);
        out
    }

    fn status(resp: &str) -> u16 {
        resp.get(9..12).and_then(|s| s.parse().ok()).unwrap_or(0)
    }

    fn post(addr: SocketAddr, event: &str, body: &str, sig: &str) -> u16 {
        let mut req = format!(
            "POST /hook HTTP/1.1\r\nHost: x\r\nX-Gitea-Event: {event}\r\nContent-Length: {}\r\n",
            body.len()
        );
        if !sig.is_empty() {
            req.push_str(&format!("X-Gitea-Signature: {sig}\r\n"));
        }
        req.push_str("\r\n");
        req.push_str(body);
        status(&raw(addr, req.as_bytes()))
    }

    #[test]
    fn listener() {
        let (srv, got) = start();
        let a = srv.local_addr();
        let body = r#"{"repository":{"id":1,"full_name":"alice/site"}}"#;
        assert_eq!(post(a, "push", body, ""), 403, "unsigned");
        assert_eq!(post(a, "push", body, "deadbeef"), 403, "forged");
        assert_eq!(
            post(a, "push", "not json", &sign("s3cret", "not json")),
            400,
            "bad json"
        );
        assert_eq!(post(a, "push", body, &sign("s3cret", body)), 202, "ok");
        assert_eq!(
            post(a, "issues", body, &sign("s3cret", body)),
            202,
            "ignored"
        );
        let got = got.lock().unwrap().clone();
        assert_eq!(
            got,
            vec![Event {
                kind: Change::Changed,
                owner: "alice".into(),
                name: "site".into(),
                id: 1
            }]
        );
        let health = raw(a, b"GET /health HTTP/1.1\r\nHost: x\r\n\r\n");
        assert_eq!(status(&health), 200);
        assert!(health.ends_with("\r\n\r\nok\n"));
        assert_eq!(
            status(&raw(a, b"GET /hook HTTP/1.1\r\nHost: x\r\n\r\n")),
            404
        );
        srv.close();
    }

    #[test]
    fn chunked_bodies_and_expect_continue() {
        let (srv, got) = start();
        let body = r#"{"repository":{"id":2,"full_name":"bob/app"}}"#;
        let (a, b) = body.split_at(10);
        let req = format!(
            "POST /hook HTTP/1.1\r\nHost: x\r\nExpect: 100-continue\r\nTransfer-Encoding: chunked\r\nX-Gitea-Event: push\r\nX-Gitea-Signature: {}\r\n\r\n{:x}\r\n{a}\r\n{:x};ext=1\r\n{b}\r\n0\r\n\r\n",
            sign("s3cret", body),
            a.len(),
            b.len()
        );
        let resp = raw(srv.local_addr(), req.as_bytes());
        assert!(
            resp.starts_with("HTTP/1.1 100 Continue\r\n\r\nHTTP/1.1 202"),
            "{resp}"
        );
        assert_eq!(got.lock().unwrap()[0].id, 2);
    }

    #[test]
    fn limits() {
        let (srv, _) = start();
        let a = srv.local_addr();
        let big = format!(
            "POST /hook HTTP/1.1\r\nContent-Length: {}\r\n\r\n",
            MAX_BODY + 1
        );
        assert_eq!(status(&raw(a, big.as_bytes())), 413);
        let huge_header = format!(
            "GET /health HTTP/1.1\r\nX-Big: {}\r\n\r\n",
            "a".repeat(MAX_HEADER + 10)
        );
        assert_eq!(status(&raw(a, huge_header.as_bytes())), 431);
        assert_eq!(status(&raw(a, b"NONSENSE\r\n\r\n")), 400);
        let smuggle = b"POST /hook HTTP/1.1\r\nContent-Length: 3\r\nTransfer-Encoding: chunked\r\n\r\n0\r\n\r\n";
        assert_eq!(status(&raw(a, smuggle)), 400);
        let twice = b"POST /hook HTTP/1.1\r\nContent-Length: 3\r\nContent-Length: 4\r\n\r\nabcd";
        assert_eq!(status(&raw(a, twice)), 400);
        let short = b"POST /hook HTTP/1.1\r\nContent-Length: 10\r\n\r\nabc";
        let mut s = TcpStream::connect(a).unwrap();
        s.write_all(short).unwrap();
        s.shutdown(Shutdown::Write).unwrap();
        let mut out = String::new();
        let _ = s.read_to_string(&mut out);
        assert!(out.is_empty(), "a truncated request gets no answer: {out}");
    }

    fn exchange(addr: SocketAddr, req: &[u8]) -> (String, Duration) {
        let start = Instant::now();
        let mut s = TcpStream::connect(addr).unwrap();
        s.set_read_timeout(Some(Duration::from_secs(20))).unwrap();
        let _ = s.write_all(req);
        let mut out = Vec::new();
        let _ = s.read_to_end(&mut out);
        (String::from_utf8_lossy(&out).into_owned(), start.elapsed())
    }

    #[test]
    fn chunk_sizes_cannot_overflow_the_body_limit() {
        let (srv, got) = start();
        let req = b"POST /hook HTTP/1.1\r\nTransfer-Encoding: chunked\r\n\r\n1\r\nA\r\nffffffffffffffff\r\n";
        let (resp, took) = exchange(srv.local_addr(), req);
        assert_eq!(status(&resp), 413, "{resp}");
        assert!(took < Duration::from_secs(5), "{took:?}");
        let (resp, _) = exchange(
            srv.local_addr(),
            b"POST /hook HTTP/1.1\r\nTransfer-Encoding: chunked\r\n\r\n+5\r\nhello\r\n0\r\n\r\n",
        );
        assert_eq!(status(&resp), 400, "{resp}");
        assert!(got.lock().unwrap().is_empty());
    }

    #[test]
    fn endless_trailers_and_blank_lines_are_cut_off() {
        let (srv, _) = start();
        let mut req = b"POST /hook HTTP/1.1\r\nTransfer-Encoding: chunked\r\n\r\n0\r\n".to_vec();
        for _ in 0..(MAX_HEADER / 5 + 10) {
            req.extend_from_slice(b"a:b\r\n");
        }
        let (resp, took) = exchange(srv.local_addr(), &req);
        assert_eq!(status(&resp), 431, "{resp}");
        assert!(took < Duration::from_secs(5), "{took:?}");
        let blank = "\r\n".repeat(MAX_HEADER / 2 + 10);
        let (resp, took) = exchange(srv.local_addr(), blank.as_bytes());
        assert_eq!(status(&resp), 431, "{resp}");
        assert!(took < Duration::from_secs(5), "{took:?}");
    }

    #[test]
    fn one_byte_budget_covers_the_whole_request() {
        let ln = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = ln.local_addr().unwrap();
        let client = thread::spawn(move || {
            let mut s = TcpStream::connect(addr).unwrap();
            let mut req = b"POST /hook HTTP/1.1\r\nTransfer-Encoding: chunked\r\n\r\n".to_vec();
            let pad = "p".repeat(1000);
            for _ in 0..200 {
                req.extend_from_slice(format!("1;{pad}\r\nx\r\n").as_bytes());
            }
            let _ = s.write_all(&req);
        });
        let (conn, _) = ln.accept().unwrap();
        let err = read_request(&conn, 1000).unwrap_err();
        assert_eq!(err, ReadError::BodyTooLarge);
        drop(conn);
        let _ = client.join();
    }

    #[test]
    fn error_replies_do_not_hold_the_connection() {
        let (srv, _) = start();
        let start = Instant::now();
        let mut s = TcpStream::connect(srv.local_addr()).unwrap();
        s.write_all(
            format!(
                "POST /hook HTTP/1.1\r\nContent-Length: {}\r\n\r\n",
                MAX_BODY + 1
            )
            .as_bytes(),
        )
        .unwrap();
        let mut closed = false;
        while start.elapsed() < Duration::from_secs(8) {
            thread::sleep(Duration::from_millis(200));
            if s.write_all(b"x").is_err() {
                closed = true;
                break;
            }
        }
        assert!(closed, "a trickling client must be dropped");
        assert!(
            start.elapsed() < Duration::from_secs(5),
            "{:?}",
            start.elapsed()
        );
    }

    #[test]
    fn close_waits_for_handlers_in_flight() {
        let (srv, _) = start();
        let slot = Slot::take(&srv.live, 2).unwrap();
        let t = thread::spawn(move || {
            thread::sleep(Duration::from_millis(200));
            drop(slot);
        });
        let began = Instant::now();
        assert!(srv.close_within(Duration::from_secs(5)));
        assert!(began.elapsed() >= Duration::from_millis(150));
        t.join().unwrap();

        let (srv, _) = start();
        let _held = Slot::take(&srv.live, 2).unwrap();
        assert!(!srv.close_within(Duration::from_millis(50)));
    }

    #[test]
    fn slots_are_capped_and_released_even_on_panic() {
        let active = Arc::new(Live::default());
        let a = Slot::take(&active, 2).unwrap();
        let b = Slot::take(&active, 2).unwrap();
        assert!(Slot::take(&active, 2).is_none());
        drop(a);
        let t = thread::spawn(move || {
            let _b = b;
            panic!("boom");
        });
        assert!(t.join().is_err());
        assert_eq!(active.count(), 0);
    }

    #[test]
    fn a_panicking_provider_gets_a_500_and_the_server_keeps_going() {
        let (srv, _) = start();
        let (resp, _) = exchange(
            srv.local_addr(),
            b"POST /hook HTTP/1.1\r\nX-Panic: 1\r\nContent-Length: 2\r\n\r\n{}",
        );
        assert_eq!(status(&resp), 500, "{resp}");
        let health = raw(srv.local_addr(), b"GET /health HTTP/1.1\r\n\r\n");
        assert_eq!(status(&health), 200);
    }

    #[test]
    fn busy_listener_answers_503() {
        let ln = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = ln.local_addr().unwrap();
        let c = TcpStream::connect(addr).unwrap();
        let (s, _) = ln.accept().unwrap();
        busy(s);
        let mut out = String::new();
        let _ = (&c).read_to_string(&mut out);
        assert_eq!(status(&out), 503, "{out}");
        assert!(out.contains("Connection: close"));
    }

    #[test]
    fn utf8_header_values_are_accepted() {
        let (srv, got) = start();
        let body = r#"{"repository":{"id":3,"full_name":"carol/web"}}"#;
        let req = format!(
            "POST /hook HTTP/1.1\r\nUser-Agent: Forgejo \u{e9}t\u{e9}\r\nX-Gitea-Event: push\r\nX-Gitea-Signature: {}\r\nContent-Length: {}\r\n\r\n{body}",
            sign("s3cret", body),
            body.len()
        );
        assert_eq!(status(&raw(srv.local_addr(), req.as_bytes())), 202);
        assert_eq!(got.lock().unwrap()[0].id, 3);
        let mut latin1 = b"GET /health HTTP/1.1\r\nX-Name: caf".to_vec();
        latin1.extend_from_slice(b"\xe9\r\n\r\n");
        assert_eq!(status(&raw(srv.local_addr(), &latin1)), 200);
    }

    #[test]
    fn listen_addresses_prefer_ipv4_and_cover_both_stacks() {
        let local = bind_addrs("localhost", 9001).unwrap();
        assert!(local[0].is_ipv4(), "{local:?}");
        let any = bind_addrs("", 9001).unwrap();
        assert!(any.iter().any(|a| a.is_ipv4() && a.ip().is_unspecified()));
        if dual_stack() {
            assert!(any[0].is_ipv6() && any[0].ip().is_unspecified(), "{any:?}");
        }
        logx::silence();
        let srv = listen("", 0, Handler::new(Arc::new(Fake), String::new, |_| {})).unwrap();
        let port = srv.local_addr().port();
        let health = raw(
            SocketAddr::from(([127, 0, 0, 1], port)),
            b"GET /health HTTP/1.1\r\n\r\n",
        );
        assert_eq!(status(&health), 200);
    }

    #[test]
    fn listen_errors_are_config_errors() {
        let (srv, _) = start();
        let port = i64::from(srv.local_addr().port());
        let h = Handler::new(Arc::new(Fake), String::new, |_| {});
        let err = listen("127.0.0.1", port, h).err().unwrap();
        assert!(err.is_config());
        assert!(
            err.to_string()
                .starts_with(&format!("cannot listen on 127.0.0.1:{port}: "))
        );
    }
}
