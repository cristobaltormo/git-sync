#[cfg(unix)]
#[path = "proc_unix.rs"]
mod proc;
#[cfg(windows)]
#[path = "proc_windows.rs"]
mod proc;

use std::ffi::OsString;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use sha1::{Digest, Sha1};

use crate::config::Config;
use crate::error::Error;
use crate::forge::{Provider, Repo, hex};
use crate::httpx::base64;
use crate::logx;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Options {
    pub tags: bool,
    pub prune: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Outcome {
    pub summary: String,
    pub refs: String,
}

const GIT_OPTS: &[&str] = &[
    "-c",
    "pack.threads=1",
    "-c",
    "pack.windowMemory=64m",
    "-c",
    "pack.deltaCacheSize=32m",
    "-c",
    "core.packedGitLimit=128m",
    "-c",
    "gc.autoDetach=false",
    "-c",
    "http.lowSpeedLimit=1000",
    "-c",
    "http.lowSpeedTime=60",
];

const DEV_NULL: &str = if cfg!(windows) { "NUL" } else { "/dev/null" };

pub struct Git {
    src: Arc<dyn Provider>,
    dry: bool,
    root: PathBuf,
    github_token: String,
    github_git_url: String,
    source_scope: String,
    timeout: Option<Duration>,
    stall: Duration,
    wait_delay: Duration,
    program: OsString,
}

enum Piped {
    Out(Vec<u8>),
    Err,
}

#[derive(Debug)]
enum RunError {
    Stalled(String),
    Failed(Error),
}

impl From<RunError> for Error {
    fn from(e: RunError) -> Error {
        match e {
            RunError::Stalled(m) => Error::Other(m),
            RunError::Failed(e) => e,
        }
    }
}

struct Tail {
    last: Instant,
    lines: Vec<String>,
    buf: Vec<u8>,
}

impl Tail {
    fn new() -> Tail {
        Tail {
            last: Instant::now(),
            lines: Vec::new(),
            buf: Vec::new(),
        }
    }

    fn write(&mut self, b: &[u8]) {
        self.last = Instant::now();
        self.buf.extend_from_slice(b);
        let mut start = 0;
        while let Some(i) = self.buf[start..]
            .iter()
            .position(|&c| c == b'\r' || c == b'\n')
        {
            let line = String::from_utf8_lossy(&self.buf[start..start + i]);
            let line = line.trim();
            if !line.is_empty() {
                if self.lines.len() == 3 {
                    self.lines.remove(0);
                }
                self.lines.push(line.to_string());
            }
            start += i + 1;
        }
        self.buf.drain(..start);
    }

    fn text(&self) -> String {
        self.lines.join(" | ")
    }
}

impl Git {
    pub fn new(cfg: &Config, src: Arc<dyn Provider>, dry: bool) -> Git {
        logx::hide(&base64(
            format!("x-access-token:{}", cfg.github.token).as_bytes(),
        ));
        let timeout =
            (cfg.sync.git_timeout > 0).then(|| Duration::from_secs(cfg.sync.git_timeout as u64));
        Git {
            src,
            dry,
            root: Path::new(&cfg.paths.state_dir).join("repos"),
            github_token: cfg.github.token.clone(),
            github_git_url: cfg.github.git_url.trim_end_matches('/').to_string(),
            source_scope: cfg.source_scope(),
            timeout,
            stall: Duration::from_secs(15),
            wait_delay: Duration::from_secs(5),
            program: OsString::from("git"),
        }
    }

    pub fn environment(&self) -> Result<Vec<(String, String)>, Error> {
        let header = self.src.git_header()?;
        logx::hide(header.strip_prefix("Authorization: ").unwrap_or(&header));
        let basic = base64(format!("x-access-token:{}", self.github_token).as_bytes());
        let pairs = [
            ("GIT_TERMINAL_PROMPT", "0".to_string()),
            ("GIT_CONFIG_GLOBAL", DEV_NULL.to_string()),
            ("GIT_CONFIG_SYSTEM", DEV_NULL.to_string()),
            ("GIT_CONFIG_COUNT", "2".to_string()),
            (
                "GIT_CONFIG_KEY_0",
                format!("http.{}.extraheader", self.source_scope),
            ),
            ("GIT_CONFIG_VALUE_0", header),
            (
                "GIT_CONFIG_KEY_1",
                format!("http.{}/.extraheader", self.github_git_url),
            ),
            (
                "GIT_CONFIG_VALUE_1",
                format!("Authorization: Basic {basic}"),
            ),
        ];
        Ok(pairs.into_iter().map(|(k, v)| (k.to_string(), v)).collect())
    }

    fn bare(&self, id: i64) -> PathBuf {
        self.root.join(format!("{id}.git"))
    }

    fn run(
        &self,
        env: &[(String, String)],
        dir: Option<&Path>,
        args: &[&str],
    ) -> Result<String, RunError> {
        let verb = args.first().copied().unwrap_or("");
        let mut cmd = Command::new(&self.program);
        cmd.args(GIT_OPTS)
            .args(args)
            .envs(env.iter().map(|(k, v)| (k, v)))
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        if let Some(d) = dir {
            cmd.current_dir(d);
        }
        proc::group(&mut cmd);
        let mut child = match cmd.spawn() {
            Ok(c) => c,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound && dir.is_none_or(Path::is_dir) => {
                return Err(RunError::Failed(Error::other("git is not installed")));
            }
            Err(e) => return Err(RunError::Failed(e.into())),
        };
        let pid = child.id();
        let (Some(mut stdout), Some(mut stderr)) = (child.stdout.take(), child.stderr.take())
        else {
            proc::kill_tree(pid);
            let _ = child.wait();
            return Err(RunError::Failed(Error::other(
                "cannot read the output of git",
            )));
        };
        let tail = Arc::new(Mutex::new(Tail::new()));
        let (tx, rx) = mpsc::channel::<Piped>();
        let mut pending = 0;
        let sink = tail.clone();
        let err_tx = tx.clone();
        let spawned = thread::Builder::new()
            .name("git-stderr".into())
            .stack_size(32 * 1024)
            .spawn(move || {
                let mut buf = [0u8; 4096];
                loop {
                    match stderr.read(&mut buf) {
                        Ok(0) => break,
                        Ok(n) => sink
                            .lock()
                            .unwrap_or_else(|e| e.into_inner())
                            .write(&buf[..n]),
                        Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
                        Err(_) => break,
                    }
                }
                let _ = err_tx.send(Piped::Err);
            });
        pending += usize::from(spawned.is_ok());
        let spawned = thread::Builder::new()
            .name("git-stdout".into())
            .stack_size(32 * 1024)
            .spawn(move || {
                let mut out = Vec::new();
                let _ = stdout.read_to_end(&mut out);
                let _ = tx.send(Piped::Out(out));
            });
        pending += usize::from(spawned.is_ok());
        let start = Instant::now();
        let tick = (self.stall / 10).max(Duration::from_millis(10));
        let mut killed = (pending < 2).then(|| {
            proc::kill_tree(pid);
            Instant::now()
        });
        let (mut stalled, mut timed_out) = (false, false);
        let mut out = Vec::new();
        let mut status = None;
        let mut nap = Duration::from_millis(1);
        loop {
            if pending == 0 {
                match child.try_wait() {
                    Ok(Some(s)) => {
                        status = Some(Ok(s));
                        break;
                    }
                    Ok(None) => {}
                    Err(e) => {
                        status = Some(Err(e));
                        break;
                    }
                }
            }
            match killed {
                Some(at) if at.elapsed() >= self.wait_delay => break,
                Some(_) => {}
                None => {
                    let silent = tail
                        .lock()
                        .unwrap_or_else(|e| e.into_inner())
                        .last
                        .elapsed();
                    stalled = silent > self.stall;
                    timed_out = !stalled && self.timeout.is_some_and(|t| start.elapsed() > t);
                    if stalled || timed_out {
                        proc::kill_tree(pid);
                        killed = Some(Instant::now());
                        continue;
                    }
                }
            }
            if pending == 0 {
                thread::sleep(nap);
                nap = (nap * 2).min(tick);
                continue;
            }
            match rx.recv_timeout(tick) {
                Ok(Piped::Out(o)) => {
                    out = o;
                    pending -= 1;
                }
                Ok(Piped::Err) => pending -= 1,
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => pending = 0,
            }
        }
        if status.is_none() {
            status = match child.try_wait() {
                Ok(Some(s)) => Some(Ok(s)),
                Ok(None) => None,
                Err(e) => Some(Err(e)),
            };
        }
        if status.is_none() {
            crate::warnf!(
                "git {verb}: still running {} after being killed, leaving it behind",
                logx::duration(self.wait_delay)
            );
        }
        if stalled {
            return Err(RunError::Stalled(format!(
                "git {verb}: stalled for {}",
                logx::duration(self.stall)
            )));
        }
        if timed_out {
            return Err(RunError::Failed(crate::err!("git {verb} timed out")));
        }
        if !status
            .as_ref()
            .is_some_and(|s| s.as_ref().is_ok_and(|s| s.success()))
        {
            let mut msg = tail.lock().unwrap_or_else(|e| e.into_inner()).text();
            if msg.is_empty() {
                msg = String::from_utf8_lossy(&out).trim().to_string();
            }
            return Err(RunError::Failed(crate::err!("git {verb} failed: {msg}")));
        }
        Ok(String::from_utf8_lossy(&out).into_owned())
    }

    fn attempt(
        &self,
        env: &[(String, String)],
        dir: Option<&Path>,
        args: &[&str],
    ) -> Result<String, Error> {
        let mut last = None;
        for _ in 0..3 {
            match self.run(env, dir, args) {
                Err(RunError::Stalled(msg)) => {
                    crate::warnf!("{msg}, trying again");
                    last = Some(msg);
                }
                other => return other.map_err(Error::from),
            }
        }
        Err(Error::Other(last.unwrap_or_default()))
    }

    pub fn mirror(
        &self,
        r: &Repo,
        gh_owner: &str,
        gh_name: &str,
        last: &str,
        opt: Options,
    ) -> Result<Outcome, Error> {
        if self.dry {
            crate::infof!("dry-run: mirror {} -> {gh_owner}/{gh_name}", r.full());
            return Ok(Outcome {
                summary: "dry-run".into(),
                refs: String::new(),
            });
        }
        let env = self.environment()?;
        let bare = self.bare(r.id);
        let dst = format!("{}/{gh_owner}/{gh_name}.git", self.github_git_url);
        let out = self.update(r, &env, &bare, &dst, last, opt);
        if let Some(state_dir) = self.root.parent() {
            crate::config::like_dir(&self.root, state_dir);
            crate::config::tree_like_dir(&bare, state_dir);
        }
        out
    }

    fn update(
        &self,
        r: &Repo,
        env: &[(String, String)],
        bare: &Path,
        dst: &str,
        last: &str,
        opt: Options,
    ) -> Result<Outcome, Error> {
        if std::fs::metadata(bare).is_err() {
            std::fs::create_dir_all(bare).map_err(|e| {
                crate::err!("mkdir {}: {}", bare.display(), crate::error::io_message(&e))
            })?;
            let path = bare.to_string_lossy();
            self.run(env, None, &["init", "--bare", "-q", &path])?;
        }
        let mut refs = vec!["+refs/heads/*:refs/heads/*"];
        let mut kinds = vec!["refs/heads"];
        if opt.tags {
            refs.push("+refs/tags/*:refs/tags/*");
            kinds.push("refs/tags");
        }
        let start = Instant::now();
        let clone_url = self.src.clone_url(r);
        let mut fetch = vec![
            "fetch",
            "--progress",
            "--prune",
            "--update-head-ok",
            &clone_url,
        ];
        fetch.extend_from_slice(&refs);
        self.attempt(env, Some(bare), &fetch)?;
        let fetched = start.elapsed();
        let mut list = vec!["for-each-ref", "--format=%(objectname) %(refname)"];
        list.extend_from_slice(&kinds);
        let listing = self.run(env, Some(bare), &list)?;
        let state = hex(&Sha1::digest(listing.as_bytes()));
        if !last.is_empty() && last == state {
            crate::debugf!("{}: nothing new since the last push", r.full());
            return Ok(Outcome {
                summary: "up to date".into(),
                refs: state,
            });
        }
        let mut push = vec!["push", "--progress", "--porcelain"];
        if opt.prune {
            push.push("--prune");
        }
        push.push(dst);
        push.extend_from_slice(&refs);
        let out = self.attempt(env, Some(bare), &push)?;
        crate::debugf!(
            "{}: fetch took {}, push took {}",
            r.full(),
            logx::duration(round_ms(fetched)),
            logx::duration(round_ms(start.elapsed().saturating_sub(fetched)))
        );
        let (mut changed, mut rejected) = (0, 0);
        for line in out.split('\n') {
            match line.as_bytes().first() {
                Some(b'*' | b'+' | b'-' | b' ') => changed += 1,
                Some(b'!') => rejected += 1,
                _ => {}
            }
        }
        if rejected > 0 {
            return Err(crate::err!("GitHub rejected {rejected} ref(s)"));
        }
        let summary = if changed == 0 {
            "up to date".to_string()
        } else {
            format!("{changed} ref(s) updated")
        };
        Ok(Outcome {
            summary,
            refs: state,
        })
    }

    pub fn forget(&self, id: i64) {
        let _ = std::fs::remove_dir_all(self.bare(id));
    }
}

fn round_ms(d: Duration) -> Duration {
    Duration::from_millis(((d.as_micros() + 500) / 1000) as u64)
}

impl crate::engine::Mirror for Git {
    fn mirror(
        &self,
        r: &Repo,
        gh_owner: &str,
        gh_name: &str,
        last: &str,
        opt: Options,
    ) -> Result<Outcome, Error> {
        Git::mirror(self, r, gh_owner, gh_name, last, opt)
    }

    fn forget(&self, id: i64) {
        Git::forget(self, id)
    }
}

#[cfg(test)]
mod tests;
