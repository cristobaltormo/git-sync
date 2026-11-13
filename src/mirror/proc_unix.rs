use std::process::Command;

pub(super) fn group(cmd: &mut Command) {
    use std::os::unix::process::CommandExt;
    cmd.process_group(0);
}

pub(super) fn kill_tree(pid: u32) {
    if let Ok(pid) = i32::try_from(pid)
        && pid > 0
    {
        unsafe {
            libc::kill(-pid, libc::SIGKILL);
        }
    }
}
