use super::inventory::inventory;
use super::signals::Signals;
#[cfg(any(unix, test))]
use super::tui_model::Key;
use super::tui_model::TuiModel;
use super::{build, open_app};
use crate::config::Config;
use crate::error::Error;
use std::sync::Once;
use std::sync::atomic::{AtomicBool, Ordering};

pub(super) fn cmd_repos_tui(cfg_path: &str, args: &[String]) -> Result<i32, Error> {
    let (a, _) = open_app(cfg_path, args, |_| {})?;
    eprint!("Reading the repositories...");
    let items = inventory(&a);
    eprint!("\r\x1b[K");
    let items = items?;
    let path = a.cfg.path.clone();
    let mut m = TuiModel::new(a.cfg.clone(), items);
    drop(a);
    m.refresh = Some(Box::new(move || {
        let next = build(Config::load(&path)?)?;
        let fresh = inventory(&next)?;
        Ok((next.cfg.clone(), fresh))
    }));
    run_tui(&mut m).map_err(|e| crate::err!("{}", crate::error::io_message(&e)))?;
    Ok(0)
}

fn run_tui(m: &mut TuiModel) -> std::io::Result<()> {
    if let Ok(sig) = Signals::terminal_close() {
        let _ = std::thread::Builder::new()
            .name("signals".into())
            .stack_size(64 * 1024)
            .spawn(move || {
                sig.wait();
                term::leave();
                std::process::exit(0);
            });
    }
    install_panic_hook();
    ACTIVE.store(true, Ordering::SeqCst);
    let r = term::run(m);
    ACTIVE.store(false, Ordering::SeqCst);
    r
}

static ACTIVE: AtomicBool = AtomicBool::new(false);

fn install_panic_hook() {
    static ONCE: Once = Once::new();
    ONCE.call_once(|| {
        let prev = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            if ACTIVE.swap(false, Ordering::SeqCst) {
                term::leave();
            }
            prev(info);
        }));
    });
}

fn size_or_default(size: Option<(u16, u16)>) -> (isize, isize) {
    match size {
        Some((w, h)) if w >= 20 => (w as isize, h as isize),
        _ => (100, 30),
    }
}

#[cfg(unix)]
mod term {
    use std::io::{self, BufWriter, Write};
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicI32, Ordering};

    use super::super::tui_model::TuiModel;
    use super::{parse_keys, size_or_default};

    static SAVED: Mutex<Option<libc::termios>> = Mutex::new(None);
    static WINCH_FD: AtomicI32 = AtomicI32::new(-1);

    extern "C" fn on_winch(_: libc::c_int) {
        let fd = WINCH_FD.load(Ordering::Relaxed);
        if fd >= 0 {
            let b = 0u8;
            unsafe {
                libc::write(fd, (&raw const b).cast(), 1);
            }
        }
    }

    fn winch_pipe() -> io::Result<libc::c_int> {
        let mut fds = [0 as libc::c_int; 2];
        if unsafe { libc::pipe(fds.as_mut_ptr()) } != 0 {
            return Err(io::Error::last_os_error());
        }
        unsafe {
            for fd in fds {
                libc::fcntl(fd, libc::F_SETFD, libc::FD_CLOEXEC);
                let fl = libc::fcntl(fd, libc::F_GETFL);
                libc::fcntl(fd, libc::F_SETFL, fl | libc::O_NONBLOCK);
            }
            WINCH_FD.store(fds[1], Ordering::SeqCst);
            let mut sa: libc::sigaction = std::mem::zeroed();
            sa.sa_sigaction = on_winch as extern "C" fn(libc::c_int) as libc::sighandler_t;
            sa.sa_flags = libc::SA_RESTART;
            libc::sigemptyset(&mut sa.sa_mask);
            libc::sigaction(libc::SIGWINCH, &sa, std::ptr::null_mut());
        }
        Ok(fds[0])
    }

    fn size() -> (isize, isize) {
        let mut ws: libc::winsize = unsafe { std::mem::zeroed() };
        let ok = unsafe { libc::ioctl(1, libc::TIOCGWINSZ, &mut ws) } == 0;
        size_or_default(ok.then_some((ws.ws_col, ws.ws_row)))
    }

    fn raw() -> io::Result<()> {
        let mut t: libc::termios = unsafe { std::mem::zeroed() };
        if unsafe { libc::tcgetattr(0, &mut t) } != 0 {
            return Err(io::Error::last_os_error());
        }
        let old = t;
        unsafe { libc::cfmakeraw(&mut t) };
        t.c_cc[libc::VMIN] = 1;
        t.c_cc[libc::VTIME] = 0;
        if unsafe { libc::tcsetattr(0, libc::TCSANOW, &t) } != 0 {
            return Err(io::Error::last_os_error());
        }
        *SAVED.lock().unwrap_or_else(|e| e.into_inner()) = Some(old);
        Ok(())
    }

    pub(super) fn leave() {
        let saved = SAVED.lock().unwrap_or_else(|e| e.into_inner()).take();
        if let Some(t) = saved {
            let mut out = io::stdout();
            let _ = out.write_all(b"\x1b[?25h\x1b[?1049l");
            let _ = out.flush();
            unsafe { libc::tcsetattr(0, libc::TCSANOW, &t) };
        }
    }

    struct Screen;

    impl Drop for Screen {
        fn drop(&mut self) {
            leave();
        }
    }

    fn wait_input(winch: libc::c_int) -> io::Result<bool> {
        let mut fds = [
            libc::pollfd {
                fd: 0,
                events: libc::POLLIN,
                revents: 0,
            },
            libc::pollfd {
                fd: winch,
                events: libc::POLLIN,
                revents: 0,
            },
        ];
        let n = if winch >= 0 { 2 } else { 1 };
        loop {
            let r = unsafe { libc::poll(fds.as_mut_ptr(), n, -1) };
            if r >= 0 {
                break;
            }
            let e = io::Error::last_os_error();
            if e.kind() != io::ErrorKind::Interrupted {
                return Err(e);
            }
        }
        if n == 2 && fds[1].revents != 0 {
            let mut drain = [0u8; 16];
            while unsafe { libc::read(winch, drain.as_mut_ptr().cast(), drain.len()) } > 0 {}
        }
        Ok(fds[0].revents != 0)
    }

    pub(super) fn run(m: &mut TuiModel) -> io::Result<()> {
        raw()?;
        let _screen = Screen;
        let winch = winch_pipe().unwrap_or(-1);
        let mut out = BufWriter::new(io::stdout());
        out.write_all(b"\x1b[?1049h\x1b[?25l")?;
        let mut buf = [0u8; 128];
        while !m.quit {
            let (w, h) = size();
            let lines = m.render(w, h);
            for i in 0..h.max(0) as usize {
                let text = lines.get(i).map_or("", String::as_str);
                write!(out, "\x1b[{};1H{text}\x1b[K", i + 1)?;
            }
            out.flush()?;
            if !wait_input(winch)? {
                continue;
            }
            let n = unsafe { libc::read(0, buf.as_mut_ptr().cast(), buf.len()) };
            if n < 0 {
                let e = io::Error::last_os_error();
                if e.kind() == io::ErrorKind::Interrupted || e.kind() == io::ErrorKind::WouldBlock {
                    continue;
                }
                return Err(e);
            }
            if n == 0 {
                return Ok(());
            }
            for k in parse_keys(&buf[..n as usize]) {
                m.press(k);
                if m.quit {
                    break;
                }
            }
        }
        Ok(())
    }
}

#[cfg(windows)]
mod term {
    use std::io::{BufWriter, Write};

    use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
    use crossterm::{cursor, queue, terminal};

    use super::super::tui_model::{Key, TuiModel};
    use super::size_or_default;

    fn map_key(k: KeyEvent, out: &mut Vec<Key>) {
        if k.kind == KeyEventKind::Release {
            return;
        }
        let ctrl = k.modifiers.contains(KeyModifiers::CONTROL);
        let key = match k.code {
            KeyCode::Char('c') if ctrl => Key::CtrlC,
            KeyCode::Char('h') if ctrl => Key::Backspace,
            KeyCode::Char('i') if ctrl => Key::Tab,
            KeyCode::Char('j' | 'm') if ctrl => Key::Enter,
            KeyCode::Char(_) if ctrl => return,
            KeyCode::Char(c) => {
                if k.modifiers.contains(KeyModifiers::ALT) {
                    out.push(Key::Esc);
                }
                Key::Rune(c)
            }
            KeyCode::Up => Key::Up,
            KeyCode::Down => Key::Down,
            KeyCode::Left => Key::Left,
            KeyCode::Right => Key::Right,
            KeyCode::PageUp => Key::PgUp,
            KeyCode::PageDown => Key::PgDn,
            KeyCode::Home => Key::Home,
            KeyCode::End => Key::End,
            KeyCode::Enter => Key::Enter,
            KeyCode::Esc => Key::Esc,
            KeyCode::Tab => Key::Tab,
            KeyCode::Backspace => Key::Backspace,
            _ => return,
        };
        out.push(key);
    }

    struct Screen;

    impl Screen {
        fn enter(out: &mut impl Write) -> std::io::Result<Screen> {
            terminal::enable_raw_mode()?;
            let s = Screen;
            queue!(out, terminal::EnterAlternateScreen, cursor::Hide)?;
            Ok(s)
        }
    }

    pub(super) fn leave() {
        let mut out = std::io::stdout();
        let _ = crossterm::execute!(out, cursor::Show, terminal::LeaveAlternateScreen);
        let _ = terminal::disable_raw_mode();
    }

    impl Drop for Screen {
        fn drop(&mut self) {
            leave();
        }
    }

    fn term_size() -> (isize, isize) {
        size_or_default(terminal::size().ok())
    }

    pub(super) fn run(m: &mut TuiModel) -> std::io::Result<()> {
        let mut out = BufWriter::new(std::io::stdout());
        let _screen = Screen::enter(&mut out)?;
        let mut keys = Vec::new();
        while !m.quit {
            let (w, h) = term_size();
            let lines = m.render(w, h);
            for i in 0..h.max(0) as usize {
                let text = lines.get(i).map_or("", String::as_str);
                queue!(
                    out,
                    cursor::MoveTo(0, i as u16),
                    crossterm::style::Print(text),
                    terminal::Clear(terminal::ClearType::UntilNewLine)
                )?;
            }
            out.flush()?;
            match event::read()? {
                Event::Key(k) => {
                    keys.clear();
                    map_key(k, &mut keys);
                    for &k in &keys {
                        m.press(k);
                        if m.quit {
                            break;
                        }
                    }
                }
                Event::Resize(..) => {}
                _ => {}
            }
        }
        Ok(())
    }
}

#[cfg(any(unix, test))]
pub(crate) fn parse_keys(b: &[u8]) -> Vec<Key> {
    let mut out = Vec::new();
    let mut b = b;
    while let Some(&c) = b.first() {
        match c {
            0x1b => {
                if b.len() >= 3 && (b[1] == b'[' || b[1] == b'O') {
                    let (k, n) = parse_escape(b);
                    if n > 0 {
                        if let Some(k) = k {
                            out.push(k);
                        }
                        b = &b[n..];
                        continue;
                    }
                }
                out.push(Key::Esc);
                b = &b[1..];
            }
            3 => {
                out.push(Key::CtrlC);
                b = &b[1..];
            }
            b'\r' | b'\n' => {
                out.push(Key::Enter);
                b = &b[1..];
            }
            b'\t' => {
                out.push(Key::Tab);
                b = &b[1..];
            }
            0x7f | 0x08 => {
                out.push(Key::Backspace);
                b = &b[1..];
            }
            c if c < 0x20 => b = &b[1..],
            _ => {
                let s = String::from_utf8_lossy(&b[..b.len().min(4)]);
                let ch = s.chars().next().unwrap_or('\u{fffd}');
                out.push(Key::Rune(ch));
                b = &b[ch.len_utf8().min(b.len())..];
            }
        }
    }
    out
}

#[cfg(any(unix, test))]
fn parse_escape(b: &[u8]) -> (Option<Key>, usize) {
    match b[2] {
        b'A' => return (Some(Key::Up), 3),
        b'B' => return (Some(Key::Down), 3),
        b'C' => return (Some(Key::Right), 3),
        b'D' => return (Some(Key::Left), 3),
        b'H' => return (Some(Key::Home), 3),
        b'F' => return (Some(Key::End), 3),
        _ => {}
    }
    if b[1] == b'[' && b[2].is_ascii_digit() {
        let mut end = 3;
        while end < b.len() && (b[end].is_ascii_digit() || b[end] == b';') {
            end += 1;
        }
        if end < b.len() {
            if b[end] == b'~' {
                let k = match b[2] {
                    b'1' | b'7' => Some(Key::Home),
                    b'4' | b'8' => Some(Key::End),
                    b'5' => Some(Key::PgUp),
                    b'6' => Some(Key::PgDn),
                    _ => None,
                };
                return (k, end + 1);
            }
            return (None, end + 1);
        }
    }
    (None, 0)
}
