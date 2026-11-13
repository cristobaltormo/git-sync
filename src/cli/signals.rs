#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Signal {
    Stop,
    Reload,
}

#[cfg(unix)]
mod imp {
    use std::io;
    use std::sync::atomic::{AtomicI32, Ordering};

    use super::Signal;

    static WRITE_FD: AtomicI32 = AtomicI32::new(-1);
    static READ_FD: AtomicI32 = AtomicI32::new(-1);

    extern "C" fn on_signal(sig: libc::c_int) {
        let fd = WRITE_FD.load(Ordering::Relaxed);
        if fd >= 0 {
            let b = sig as u8;
            unsafe {
                libc::write(fd, (&raw const b).cast(), 1);
            }
        }
    }

    fn pipe() -> io::Result<()> {
        if READ_FD.load(Ordering::SeqCst) >= 0 {
            return Ok(());
        }
        let mut fds = [0 as libc::c_int; 2];
        if unsafe { libc::pipe(fds.as_mut_ptr()) } != 0 {
            return Err(io::Error::last_os_error());
        }
        unsafe {
            for fd in fds {
                libc::fcntl(fd, libc::F_SETFD, libc::FD_CLOEXEC);
            }
            let fl = libc::fcntl(fds[1], libc::F_GETFL);
            libc::fcntl(fds[1], libc::F_SETFL, fl | libc::O_NONBLOCK);
        }
        READ_FD.store(fds[0], Ordering::SeqCst);
        WRITE_FD.store(fds[1], Ordering::SeqCst);
        Ok(())
    }

    fn catch(sig: libc::c_int) -> io::Result<()> {
        unsafe {
            let mut sa: libc::sigaction = std::mem::zeroed();
            sa.sa_sigaction = on_signal as extern "C" fn(libc::c_int) as libc::sighandler_t;
            sa.sa_flags = libc::SA_RESTART;
            libc::sigemptyset(&mut sa.sa_mask);
            if libc::sigaction(sig, &sa, std::ptr::null_mut()) != 0 {
                return Err(io::Error::last_os_error());
            }
        }
        Ok(())
    }

    pub(crate) struct Signals;

    impl Signals {
        pub(crate) fn stop_and_reload() -> io::Result<Signals> {
            Signals::install(&[libc::SIGINT, libc::SIGTERM, libc::SIGHUP])
        }

        pub(crate) fn terminal_close() -> io::Result<Signals> {
            Signals::install(&[libc::SIGTERM, libc::SIGHUP])
        }

        fn install(sigs: &[libc::c_int]) -> io::Result<Signals> {
            pipe()?;
            for &s in sigs {
                catch(s)?;
            }
            Ok(Signals)
        }

        pub(crate) fn wait(&self) -> Signal {
            let fd = READ_FD.load(Ordering::SeqCst);
            loop {
                let mut b = 0u8;
                let n = unsafe { libc::read(fd, (&raw mut b).cast(), 1) };
                if n == 1 {
                    return if i32::from(b) == libc::SIGHUP {
                        Signal::Reload
                    } else {
                        Signal::Stop
                    };
                }
                if n < 0 && io::Error::last_os_error().kind() == io::ErrorKind::Interrupted {
                    continue;
                }
                return Signal::Stop;
            }
        }
    }

    pub(crate) fn default_sigpipe() {
        unsafe {
            libc::signal(libc::SIGPIPE, libc::SIG_DFL);
        }
    }
}

#[cfg(windows)]
mod imp {
    use std::io;
    use std::sync::{Condvar, Mutex};
    use std::time::Duration;

    use windows_sys::Win32::System::Console::{
        CTRL_BREAK_EVENT, CTRL_C_EVENT, CTRL_CLOSE_EVENT, CTRL_LOGOFF_EVENT, CTRL_SHUTDOWN_EVENT,
        SetConsoleCtrlHandler,
    };
    use windows_sys::core::BOOL;

    use super::Signal;

    static RAISED: Mutex<u32> = Mutex::new(0);
    static COND: Condvar = Condvar::new();

    unsafe extern "system" fn on_ctrl(kind: u32) -> BOOL {
        match kind {
            CTRL_C_EVENT | CTRL_BREAK_EVENT | CTRL_CLOSE_EVENT | CTRL_LOGOFF_EVENT
            | CTRL_SHUTDOWN_EVENT => {
                *RAISED.lock().unwrap_or_else(|e| e.into_inner()) += 1;
                COND.notify_all();
                if kind != CTRL_C_EVENT && kind != CTRL_BREAK_EVENT {
                    std::thread::sleep(Duration::from_secs(20));
                }
                1
            }
            _ => 0,
        }
    }

    pub(crate) struct Signals;

    impl Signals {
        pub(crate) fn stop_and_reload() -> io::Result<Signals> {
            Signals::install()
        }

        pub(crate) fn terminal_close() -> io::Result<Signals> {
            Signals::install()
        }

        fn install() -> io::Result<Signals> {
            if unsafe { SetConsoleCtrlHandler(Some(on_ctrl), 1) } == 0 {
                return Err(io::Error::last_os_error());
            }
            Ok(Signals)
        }

        pub(crate) fn wait(&self) -> Signal {
            let mut n = RAISED.lock().unwrap_or_else(|e| e.into_inner());
            while *n == 0 {
                n = COND.wait(n).unwrap_or_else(|e| e.into_inner());
            }
            *n -= 1;
            Signal::Stop
        }
    }

    pub(crate) fn default_sigpipe() {}
}

pub(crate) use imp::{Signals, default_sigpipe};

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    #[test]
    fn a_raised_signal_wakes_the_waiter() {
        let s = Signals::stop_and_reload().unwrap();
        unsafe {
            libc::raise(libc::SIGHUP);
        }
        assert_eq!(s.wait(), Signal::Reload);
    }
}
