#![cfg(unix)]

use std::io::Read;
use std::net::{TcpListener, TcpStream};
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

struct BlackHole {
    port: u16,
    held: Arc<Mutex<Vec<TcpStream>>>,
}

impl BlackHole {
    fn start() -> BlackHole {
        let ln = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = ln.local_addr().unwrap().port();
        let held = Arc::new(Mutex::new(Vec::new()));
        let h = held.clone();
        thread::spawn(move || {
            for c in ln.incoming().flatten() {
                h.lock().unwrap().push(c);
            }
        });
        BlackHole { port, held }
    }

    fn accepted(&self) -> usize {
        self.held.lock().unwrap().len()
    }
}

fn free_port() -> u16 {
    TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

fn stops_quickly(mode: &str) {
    let source = BlackHole::start();
    let github = BlackHole::start();
    let dir = std::env::temp_dir().join(format!("gitsync-shutdown-{}-{mode}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let cfg = dir.join("config.toml");
    std::fs::write(
        &cfg,
        format!(
            "[source]\ntype = \"forgejo\"\nurl = \"http://127.0.0.1:{}\"\ntoken = \"x\"\n\
             [github]\ntoken = \"y\"\napi_url = \"http://127.0.0.1:{}\"\n\
             [accounts]\nalice = \"alice\"\n\
             [listen]\nhost = \"127.0.0.1\"\nport = {}\nsecret = \"s\"\n\
             [hooks]\nmode = \"{mode}\"\n\
             [paths]\nstate_dir = \"{}\"\n",
            source.port,
            github.port,
            free_port(),
            dir.join("state").display()
        ),
    )
    .unwrap();
    let mut child = Command::new(env!("CARGO_BIN_EXE_gitsync"))
        .args(["-c", cfg.to_str().unwrap(), "run"])
        .env_remove("GITSYNC_CONFIG")
        .env("NO_COLOR", "1")
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(10);
    while source.accepted() == 0 && Instant::now() < deadline {
        thread::sleep(Duration::from_millis(20));
    }
    assert!(source.accepted() > 0, "the source was never contacted");
    thread::sleep(Duration::from_millis(300));
    let start = Instant::now();
    unsafe {
        libc::kill(child.id() as libc::pid_t, libc::SIGTERM);
    }
    let status = loop {
        if let Some(s) = child.try_wait().unwrap() {
            break s;
        }
        if start.elapsed() > Duration::from_secs(20) {
            let _ = child.kill();
            panic!("gitsync did not stop within 20 s (hooks.mode = {mode})");
        }
        thread::sleep(Duration::from_millis(20));
    };
    let took = start.elapsed();
    let mut log = String::new();
    child
        .stderr
        .take()
        .unwrap()
        .read_to_string(&mut log)
        .unwrap();
    let _ = std::fs::remove_dir_all(&dir);
    assert_eq!(status.code(), Some(0), "{log}");
    assert!(log.contains("stopping"), "{log}");
    assert!(
        took < Duration::from_secs(3),
        "stopping took {took:?} with hooks.mode = {mode}\n{log}"
    );
}

#[test]
fn stops_quickly_while_the_poller_waits_on_a_silent_source() {
    stops_quickly("none");
}

#[test]
fn stops_quickly_while_setting_up_hooks_on_a_silent_source() {
    stops_quickly("auto");
}
