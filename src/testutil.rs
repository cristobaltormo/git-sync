use std::net::{SocketAddr, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use crate::webhook::{Request, read_request, write_response};

pub struct TempDir(PathBuf);

impl TempDir {
    pub fn new() -> TempDir {
        static SEQ: AtomicU32 = AtomicU32::new(0);
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.subsec_nanos())
            .unwrap_or(0);
        let p = std::env::temp_dir().join(format!(
            "gitsync-test-{}-{}-{nanos}",
            std::process::id(),
            SEQ.fetch_add(1, Ordering::SeqCst)
        ));
        std::fs::create_dir_all(&p).unwrap();
        TempDir(p)
    }

    pub fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[derive(Debug, Clone)]
pub struct Reply {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

impl Reply {
    pub fn status(status: u16) -> Reply {
        Reply {
            status,
            headers: Vec::new(),
            body: Vec::new(),
        }
    }

    pub fn json(body: &str) -> Reply {
        Reply::status(200)
            .header("Content-Type", "application/json")
            .body(body)
    }

    pub fn header(mut self, k: &str, v: &str) -> Reply {
        self.headers.push((k.to_string(), v.to_string()));
        self
    }

    pub fn body(mut self, body: &str) -> Reply {
        self.body = body.as_bytes().to_vec();
        self
    }
}

#[derive(Debug, Clone)]
pub struct Seen {
    pub method: String,
    pub target: String,
    pub authorization: String,
    pub body: Vec<u8>,
}

impl Seen {
    pub fn line(&self) -> String {
        format!("{} {}", self.method, self.target)
    }
}

type Route = dyn Fn(&Request) -> Reply + Send + Sync;

pub struct TestServer {
    addr: SocketAddr,
    seen: Arc<Mutex<Vec<Seen>>>,
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl TestServer {
    pub fn start(route: impl Fn(&Request) -> Reply + Send + Sync + 'static) -> TestServer {
        let ln = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = ln.local_addr().unwrap();
        let seen = Arc::new(Mutex::new(Vec::new()));
        let stop = Arc::new(AtomicBool::new(false));
        let route: Arc<Route> = Arc::new(route);
        let (log, flag) = (seen.clone(), stop.clone());
        let thread = thread::spawn(move || {
            for conn in ln.incoming() {
                if flag.load(Ordering::SeqCst) {
                    return;
                }
                let Ok(stream) = conn else { continue };
                let _ = stream.set_write_timeout(Some(Duration::from_secs(10)));
                let Ok(req) = read_request(&stream, 64 << 20) else {
                    continue;
                };
                log.lock().unwrap().push(Seen {
                    method: req.method.clone(),
                    target: req.target.clone(),
                    authorization: req.header("authorization").to_string(),
                    body: req.body.clone(),
                });
                let reply = route(&req);
                let headers: Vec<(&str, &str)> = reply
                    .headers
                    .iter()
                    .map(|(k, v)| (k.as_str(), v.as_str()))
                    .collect();
                let _ = write_response(&stream, reply.status, &headers, &reply.body);
            }
        });
        TestServer {
            addr,
            seen,
            stop,
            thread: Some(thread),
        }
    }

    pub fn url(&self) -> String {
        format!("http://{}", self.addr)
    }

    pub fn requests(&self) -> Vec<Seen> {
        self.seen.lock().unwrap().clone()
    }

    pub fn lines(&self) -> Vec<String> {
        self.requests().iter().map(Seen::line).collect()
    }
}

impl Drop for TestServer {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if TcpStream::connect(self.addr).is_ok()
            && let Some(t) = self.thread.take()
        {
            let _ = t.join();
        }
    }
}
