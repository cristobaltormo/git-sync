use std::sync::{Arc, Mutex, mpsc};
use std::thread;
use std::time::{Duration, Instant};

use super::signals::{Signal, Signals};
use super::{build, create_state_dir, flags};
use crate::config::{self, Config};
use crate::engine::Engine;
use crate::error::Error;
use crate::webhook;

const POLLER_STACK: usize = 1 << 20;
const STOP_GRACE: Duration = Duration::from_millis(1500);

pub(super) fn cmd_run(cfg_path: &str, args: &[String]) -> Result<i32, Error> {
    let mut dry = None;
    let (path, fs) = flags("run", cfg_path, args, |fs| {
        dry = Some(fs.bool("dry-run", "log what would happen, change nothing"));
    })?;
    let file = config::find(&path);
    let mut cfg = Config::load(&file)?;
    if dry.is_some_and(|d| fs.on(d)) {
        cfg.sync.dry_run = true;
    }
    let signals = Signals::stop_and_reload()
        .map_err(|e| crate::err!("cannot install signal handlers: {e}"))?;
    let a = build(cfg)?;
    create_state_dir(&a.cfg)?;
    a.eng.start();
    let secret_eng = a.eng.clone();
    let event_eng = a.eng.clone();
    let handler = webhook::Handler::new(
        a.src.clone(),
        move || secret_eng.config().listen.secret.clone(),
        move |ev| event_eng.handle_event(&ev),
    );
    let srv = match webhook::listen(&a.cfg.listen.host, a.cfg.listen.port, handler) {
        Ok(s) => s,
        Err(e) => {
            a.eng.shutdown();
            return Err(e);
        }
    };
    let cfg = &a.cfg;
    infof!(
        "gitsync {} up: {} {} -> GitHub, {} account(s), webhooks on {}{}",
        crate::version::version(),
        cfg.source.kind,
        cfg.source_url(),
        cfg.accounts.len(),
        config::host_port(&cfg.listen.host, cfg.listen.port),
        if a.eng.dry() { " [dry run]" } else { "" }
    );

    let r = Arc::new(Reloader {
        stamp: Mutex::new(a.cfg.stamp()),
        file,
        cur: a.cfg.clone(),
        eng: a.eng.clone(),
    });
    let poll_eng = a.eng.clone();
    let tick = r.clone();
    let (done, poller_done) = mpsc::channel::<()>();
    let spawned = thread::Builder::new()
        .name("poller".into())
        .stack_size(POLLER_STACK)
        .spawn(move || {
            let _done = done;
            poll_eng.poll_loop(|| tick.check_files());
        });
    if let Err(e) = spawned {
        srv.close();
        a.eng.shutdown();
        return Err(crate::err!("cannot start the poller: {e}"));
    }

    while signals.wait() == Signal::Reload {
        r.reload();
    }
    infof!("stopping");
    a.eng.stop();
    crate::httpx::cancel();
    let grace = Instant::now() + STOP_GRACE;
    let _ = poller_done.recv_timeout(STOP_GRACE);
    srv.close_within(grace.saturating_duration_since(Instant::now()));
    a.eng.shutdown();
    Ok(0)
}

struct Reloader {
    file: String,
    cur: Arc<Config>,
    eng: Arc<Engine>,
    stamp: Mutex<String>,
}

impl Reloader {
    fn reload(&self) {
        let mut stamp = self.stamp.lock().unwrap_or_else(|e| e.into_inner());
        let st = self.cur.stamp();
        let next = Config::load(&self.file);
        *stamp = st;
        let next = match next {
            Ok(n) => n,
            Err(e) => {
                errorf!("reload failed, keeping the old config: {e}");
                return;
            }
        };
        if next.listen.host != self.cur.listen.host || next.listen.port != self.cur.listen.port {
            warnf!("listen address changed: restart needed for that part");
        }
        self.eng.reload(next);
        infof!("config reloaded");
    }

    fn check_files(&self) {
        let changed = {
            let stamp = self.stamp.lock().unwrap_or_else(|e| e.into_inner());
            *stamp != self.cur.stamp()
        };
        if changed {
            self.reload();
        }
    }
}
