use std::fs::{File, Metadata, OpenOptions};
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

pub(super) struct TempFile {
    path: PathBuf,
    done: bool,
}

impl TempFile {
    pub(super) fn create(dir: &str, prefix: &str, suffix: &str) -> io::Result<(TempFile, File)> {
        static SEQ: AtomicU32 = AtomicU32::new(0);
        let mut last = io::Error::other("cannot create a temporary file");
        for _ in 0..100 {
            let nanos = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|d| d.subsec_nanos())
                .unwrap_or(0);
            let n = std::process::id()
                .wrapping_mul(2_654_435_761)
                .wrapping_add(nanos)
                .wrapping_add(SEQ.fetch_add(1, Ordering::Relaxed).wrapping_mul(7919));
            let path = Path::new(dir).join(format!("{prefix}{n}{suffix}"));
            let mut opts = OpenOptions::new();
            opts.write(true).create_new(true);
            #[cfg(unix)]
            std::os::unix::fs::OpenOptionsExt::mode(&mut opts, 0o600);
            match opts.open(&path) {
                Ok(f) => return Ok((TempFile { path, done: false }, f)),
                Err(e) if e.kind() == io::ErrorKind::AlreadyExists => last = e,
                Err(e) => return Err(e),
            }
        }
        Err(last)
    }

    pub(super) fn path(&self) -> &Path {
        &self.path
    }

    pub(super) fn persist(mut self, dst: &str) -> io::Result<()> {
        std::fs::rename(&self.path, dst)?;
        self.done = true;
        Ok(())
    }
}

impl Drop for TempFile {
    fn drop(&mut self) {
        if !self.done {
            let _ = std::fs::remove_file(&self.path);
        }
    }
}

#[cfg(unix)]
pub(super) fn set_mode(path: &Path, mode: u32) -> io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode))
}

#[cfg(not(unix))]
pub(super) fn set_mode(_: &Path, _: u32) -> io::Result<()> {
    Ok(())
}

#[cfg(unix)]
pub(super) fn mode(st: &Metadata) -> u32 {
    use std::os::unix::fs::PermissionsExt;
    st.permissions().mode() & 0o777
}

#[cfg(not(unix))]
pub(super) fn mode(_: &Metadata) -> u32 {
    0o644
}

#[cfg(unix)]
pub(super) fn keep_owner(path: &Path, st: &Metadata) {
    use std::os::unix::fs::MetadataExt;
    let _ = std::os::unix::fs::chown(path, Some(st.uid()), Some(st.gid()));
}

#[cfg(not(unix))]
pub(super) fn keep_owner(_: &Path, _: &Metadata) {}

#[cfg(unix)]
fn dir_owner(dir: &Path) -> Option<(u32, u32)> {
    use std::os::unix::fs::MetadataExt;
    if unsafe { libc::geteuid() } != 0 {
        return None;
    }
    let dir = if dir.as_os_str().is_empty() {
        Path::new(".")
    } else {
        dir
    };
    let st = std::fs::metadata(dir).ok()?;
    (st.uid() != 0 || st.gid() != 0).then(|| (st.uid(), st.gid()))
}

#[cfg(unix)]
fn adopt(path: &Path, st: &Metadata, (uid, gid): (u32, u32)) {
    use std::os::unix::fs::MetadataExt;
    if st.uid() != uid || st.gid() != gid {
        let _ = std::os::unix::fs::lchown(path, Some(uid), Some(gid));
    }
}

#[cfg(unix)]
pub fn like_dir(path: &Path, dir: &Path) {
    if let Some(owner) = dir_owner(dir)
        && let Ok(st) = std::fs::symlink_metadata(path)
    {
        adopt(path, &st, owner);
    }
}

#[cfg(not(unix))]
pub fn like_dir(_: &Path, _: &Path) {}

#[cfg(unix)]
mod nofollow {
    use std::ffi::{CStr, CString};
    use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
    use std::os::unix::ffi::OsStrExt;
    use std::path::{Component, Path};

    fn cstr(name: &[u8]) -> Option<CString> {
        CString::new(name).ok()
    }

    fn open_dir(at: libc::c_int, name: &CStr, follow: bool) -> Option<OwnedFd> {
        let mut flags = libc::O_RDONLY | libc::O_DIRECTORY | libc::O_CLOEXEC;
        if !follow {
            flags |= libc::O_NOFOLLOW;
        }
        let fd = unsafe { libc::openat(at, name.as_ptr(), flags) };
        (fd >= 0).then(|| unsafe { OwnedFd::from_raw_fd(fd) })
    }

    fn walk(at: &OwnedFd, name: &CStr, (uid, gid): (u32, u32)) {
        let mut st: libc::stat = unsafe { std::mem::zeroed() };
        if unsafe {
            libc::fstatat(
                at.as_raw_fd(),
                name.as_ptr(),
                &mut st,
                libc::AT_SYMLINK_NOFOLLOW,
            )
        } != 0
        {
            return;
        }
        if st.st_uid != uid || st.st_gid != gid {
            unsafe {
                libc::fchownat(
                    at.as_raw_fd(),
                    name.as_ptr(),
                    uid,
                    gid,
                    libc::AT_SYMLINK_NOFOLLOW,
                )
            };
        }
        if st.st_mode & libc::S_IFMT != libc::S_IFDIR {
            return;
        }
        let Some(dir) = open_dir(at.as_raw_fd(), name, false) else {
            return;
        };
        for child in entries(&dir) {
            walk(&dir, &child, (uid, gid));
        }
    }

    fn entries(dir: &OwnedFd) -> Vec<CString> {
        let dup = unsafe { libc::fcntl(dir.as_raw_fd(), libc::F_DUPFD_CLOEXEC, 0) };
        if dup < 0 {
            return Vec::new();
        }
        let stream = unsafe { libc::fdopendir(dup) };
        if stream.is_null() {
            unsafe { libc::close(dup) };
            return Vec::new();
        }
        let mut out = Vec::new();
        loop {
            let e = unsafe { libc::readdir(stream) };
            if e.is_null() {
                break;
            }
            let name = unsafe { CStr::from_ptr((*e).d_name.as_ptr()) };
            if name.to_bytes() != b"." && name.to_bytes() != b".." {
                out.push(name.to_owned());
            }
        }
        unsafe { libc::closedir(stream) };
        out
    }

    pub(super) fn tree(path: &Path, dir: &Path, owner: (u32, u32)) {
        let base = if dir.as_os_str().is_empty() {
            Path::new(".")
        } else {
            dir
        };
        let Ok(rel) = path.strip_prefix(dir) else {
            return;
        };
        let mut names = Vec::new();
        for c in rel.components() {
            match c {
                Component::Normal(n) => match cstr(n.as_bytes()) {
                    Some(n) => names.push(n),
                    None => return,
                },
                Component::CurDir => {}
                _ => return,
            }
        }
        let Some(last) = names.pop() else {
            return;
        };
        let Some(mut at) =
            cstr(base.as_os_str().as_bytes()).and_then(|b| open_dir(libc::AT_FDCWD, &b, true))
        else {
            return;
        };
        for n in &names {
            match open_dir(at.as_raw_fd(), n, false) {
                Some(next) => at = next,
                None => return,
            }
        }
        walk(&at, &last, owner);
    }
}

#[cfg(unix)]
pub fn tree_like_dir(path: &Path, dir: &Path) {
    if let Some(owner) = dir_owner(dir) {
        nofollow::tree(path, dir, owner);
    }
}

#[cfg(not(unix))]
pub fn tree_like_dir(_: &Path, _: &Path) {}

#[cfg(all(test, unix))]
pub(super) fn owner(st: &Metadata) -> (u32, u32) {
    use std::os::unix::fs::MetadataExt;
    (st.uid(), st.gid())
}
