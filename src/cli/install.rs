use std::path::Path;
use std::process::Command;

use crate::error::{Error, io_message};

pub(crate) trait Runner {
    fn run(&mut self, name: &str, args: &[&str]) -> Result<(), Error>;
}

pub(crate) struct System;

impl Runner for System {
    fn run(&mut self, name: &str, args: &[&str]) -> Result<(), Error> {
        let out = Command::new(name)
            .args(args)
            .output()
            .map_err(|e| crate::err!("{name} {}: {}", args.join(" "), io_message(&e)))?;
        if !out.status.success() {
            let mut text = out.stdout;
            text.extend_from_slice(&out.stderr);
            return Err(crate::err!(
                "{name} {}: {}: {}",
                args.join(" "),
                out.status,
                String::from_utf8_lossy(&text).trim()
            ));
        }
        Ok(())
    }
}

pub(crate) fn copy_file(src: &Path, dst: &Path, mode: u32) -> Result<(), Error> {
    let fail = |e: std::io::Error| crate::err!("{}: {}", dst.display(), io_message(&e));
    let mut tmp = dst.as_os_str().to_owned();
    tmp.push(".new");
    let tmp = std::path::PathBuf::from(tmp);
    let mut input = std::fs::File::open(src)
        .map_err(|e| crate::err!("open {}: {}", src.display(), io_message(&e)))?;
    let mut opts = std::fs::OpenOptions::new();
    opts.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(mode);
    }
    #[cfg(not(unix))]
    let _ = mode;
    let mut out = opts.open(&tmp).map_err(fail)?;
    std::io::copy(&mut input, &mut out).map_err(fail)?;
    out.sync_all().map_err(fail)?;
    drop(out);
    std::fs::rename(&tmp, dst).map_err(fail)
}

#[cfg(any(target_os = "linux", windows, test))]
fn copy_repos(src: &Path, dst: &Path, mode: u32) -> Result<(), Error> {
    if let Ok(old) = std::fs::read(dst) {
        let new = std::fs::read(src)
            .map_err(|e| crate::err!("open {}: {}", src.display(), io_message(&e)))?;
        if old != new {
            println!(
                "kept existing {} (not overwritten by {})",
                dst.display(),
                src.display()
            );
            return Ok(());
        }
    }
    copy_file(src, dst, mode)?;
    println!("copied {} to {}", src.display(), dst.display());
    Ok(())
}

#[cfg(any(target_os = "linux", windows, test))]
fn repos_beside(src: &Path, repos_file: &str) -> Option<std::path::PathBuf> {
    if !repos_file.is_empty() {
        return None;
    }
    let p = src.parent().unwrap_or(Path::new("")).join("repos.toml");
    p.is_file().then_some(p)
}

#[cfg(target_os = "linux")]
pub(super) use linux::{cmd_install, cmd_uninstall};
#[cfg(windows)]
pub(super) use windows::{cmd_install, cmd_uninstall};

#[cfg(not(any(target_os = "linux", windows)))]
const NO_SERVICE: &str = "install is available on Linux (systemd) and Windows; here run gitsync in the background yourself, or use Docker";

#[cfg(not(any(target_os = "linux", windows)))]
pub(super) fn cmd_install(_: &str, _: &[String]) -> Result<i32, Error> {
    Err(Error::Config(NO_SERVICE.into()))
}

#[cfg(not(any(target_os = "linux", windows)))]
pub(super) fn cmd_uninstall(_: &str, _: &[String]) -> Result<i32, Error> {
    Err(Error::Config(NO_SERVICE.into()))
}

#[cfg(any(target_os = "linux", all(test, unix)))]
pub(crate) mod linux {
    use std::path::{Path, PathBuf};

    #[cfg(target_os = "linux")]
    use super::System;
    use super::{Runner, copy_file};
    #[cfg(target_os = "linux")]
    use crate::cli::flags::{BoolFlag, FlagSet};
    #[cfg(target_os = "linux")]
    use crate::config::{self, Config};
    use crate::error::{Error, io_message};

    const UNIT: &str = "[Unit]
Description=Mirror {name} repositories to GitHub
After=network-online.target
Wants=network-online.target

[Service]
User=gitsync
ExecStart={bin} run --config {config}
ExecReload=/bin/kill -HUP $MAINPID
Restart=on-failure
RestartSec=5
StateDirectory=gitsync
ProtectSystem=strict
ProtectHome=true
PrivateTmp=true
NoNewPrivileges=true
MemoryMax=512M

[Install]
WantedBy=multi-user.target
";

    pub(crate) struct Layout {
        pub(crate) bin: PathBuf,
        pub(crate) etc: PathBuf,
        pub(crate) unit: PathBuf,
        pub(crate) bin_in_unit: String,
        pub(crate) config_in_unit: String,
    }

    impl Layout {
        pub(crate) fn system() -> Layout {
            Layout {
                bin: "/usr/local/bin/gitsync".into(),
                etc: "/etc/gitsync".into(),
                unit: "/etc/systemd/system/gitsync.service".into(),
                bin_in_unit: "/usr/local/bin/gitsync".into(),
                config_in_unit: "/etc/gitsync/config.toml".into(),
            }
        }

        fn target(&self) -> PathBuf {
            self.etc.join("config.toml")
        }
    }

    pub(crate) fn unit(source_type: &str, bin: &str, config: &str) -> String {
        let mut name = source_type.to_string();
        if let Some(first) = name.get_mut(..1) {
            first.make_ascii_uppercase();
        }
        UNIT.replace("{name}", &name)
            .replace("{bin}", bin)
            .replace("{config}", config)
    }

    #[cfg(target_os = "linux")]
    fn is_root() -> bool {
        unsafe { libc::geteuid() == 0 }
    }

    #[cfg(target_os = "linux")]
    fn user_exists(name: &str) -> bool {
        let Ok(c) = std::ffi::CString::new(name) else {
            return false;
        };
        unsafe { !libc::getpwnam(c.as_ptr()).is_null() }
    }

    fn io(path: &Path) -> impl Fn(std::io::Error) -> Error + '_ {
        move |e| crate::err!("{}: {}", path.display(), io_message(&e))
    }

    #[cfg(target_os = "linux")]
    pub(crate) fn cmd_install(cfg_path: &str, args: &[String]) -> Result<i32, Error> {
        let mut no_start = None;
        let (path, fs) = parse_flags(cfg_path, args, &mut no_start)?;
        if !is_root() {
            return Err(Error::Config(
                "install needs root (try: sudo gitsync install)".into(),
            ));
        }
        let src = config::find(&path);
        let cfg = Config::load(&src)?;
        if !user_exists("gitsync") {
            System.run(
                "useradd",
                &[
                    "--system",
                    "--home-dir",
                    "/var/lib/gitsync",
                    "--no-create-home",
                    "--shell",
                    "/usr/sbin/nologin",
                    "gitsync",
                ],
            )?;
            println!("created system user gitsync");
        }
        let self_exe = std::env::current_exe()?;
        let src = Path::new(&src);
        let repos = super::repos_beside(src, &cfg.paths.repos_file);
        install(
            &Layout::system(),
            &mut System,
            &self_exe,
            src,
            repos.as_deref(),
            &cfg.source.kind,
            no_start.is_some_and(|f| fs.on(f)),
        )
    }

    #[cfg(target_os = "linux")]
    fn parse_flags(
        cfg_path: &str,
        args: &[String],
        no_start: &mut Option<BoolFlag>,
    ) -> Result<(String, FlagSet), Error> {
        crate::cli::flags("install", cfg_path, args, |fs| {
            *no_start = Some(fs.bool("no-start", "install the unit but do not start it"));
        })
    }

    pub(crate) fn install(
        l: &Layout,
        run: &mut dyn Runner,
        self_exe: &Path,
        src: &Path,
        repos: Option<&Path>,
        source_type: &str,
        no_start: bool,
    ) -> Result<i32, Error> {
        copy_file(self_exe, &l.bin, 0o755)?;
        std::fs::create_dir_all(&l.etc).map_err(io(&l.etc))?;
        let target = l.target();
        let mut owned = vec![l.etc.to_string_lossy().into_owned()];
        owned.push(target.to_string_lossy().into_owned());
        if src != target {
            copy_file(src, &target, 0o640)?;
            if let Some(repos) = repos {
                let dst = l.etc.join("repos.toml");
                super::copy_repos(repos, &dst, 0o640)?;
                owned.push(dst.to_string_lossy().into_owned());
            }
        }
        let mut args = vec!["root:gitsync"];
        args.extend(owned.iter().map(String::as_str));
        run.run("chown", &args)?;
        set_mode(&l.etc, 0o750);
        for p in &owned[1..] {
            set_mode(Path::new(p), 0o640);
        }
        let content = unit(source_type, &l.bin_in_unit, &l.config_in_unit);
        std::fs::write(&l.unit, content).map_err(io(&l.unit))?;
        set_mode(&l.unit, 0o644);
        run.run("systemctl", &["daemon-reload"])?;
        if no_start {
            println!("unit installed: systemctl enable --now gitsync");
            return Ok(0);
        }
        run.run("systemctl", &["enable", "--now", "gitsync"])?;
        println!("service started: journalctl -u gitsync -f");
        Ok(0)
    }

    fn set_mode(p: &Path, mode: u32) {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(p, std::fs::Permissions::from_mode(mode));
    }

    #[cfg(target_os = "linux")]
    pub(crate) fn cmd_uninstall(_: &str, _: &[String]) -> Result<i32, Error> {
        if !is_root() {
            return Err(Error::Config("uninstall needs root".into()));
        }
        uninstall(&Layout::system(), &mut System);
        Ok(0)
    }

    pub(crate) fn uninstall(l: &Layout, run: &mut dyn Runner) {
        let _ = run.run("systemctl", &["disable", "--now", "gitsync"]);
        let _ = std::fs::remove_file(&l.unit);
        let _ = std::fs::remove_file(&l.bin);
        let _ = run.run("systemctl", &["daemon-reload"]);
        println!("removed the service and the binary; /etc/gitsync and /var/lib/gitsync are kept");
    }
}

#[cfg(any(windows, test))]
pub(crate) mod task {
    #[cfg(windows)]
    pub(crate) const TASK_NAME: &str = "gitsync";

    const TASK_XML: &str = r#"<?xml version="1.0" encoding="UTF-16"?>
<Task version="1.2" xmlns="http://schemas.microsoft.com/windows/2004/02/mit/task">
  <RegistrationInfo><Description>Mirror git repositories to GitHub</Description></RegistrationInfo>
  <Triggers>
    <BootTrigger><Enabled>true</Enabled></BootTrigger>
    <TimeTrigger>
      <Repetition><Interval>PT1M</Interval><StopAtDurationEnd>false</StopAtDurationEnd></Repetition>
      <StartBoundary>{start}</StartBoundary>
      <Enabled>true</Enabled>
    </TimeTrigger>
  </Triggers>
  <Principals><Principal id="Author"><UserId>S-1-5-18</UserId><RunLevel>HighestAvailable</RunLevel></Principal></Principals>
  <Settings>
    <MultipleInstancesPolicy>IgnoreNew</MultipleInstancesPolicy>
    <DisallowStartIfOnBatteries>false</DisallowStartIfOnBatteries>
    <StopIfGoingOnBatteries>false</StopIfGoingOnBatteries>
    <StartWhenAvailable>true</StartWhenAvailable>
    <ExecutionTimeLimit>PT0S</ExecutionTimeLimit>
    <RestartOnFailure><Interval>PT1M</Interval><Count>999</Count></RestartOnFailure>
    <Enabled>true</Enabled>
  </Settings>
  <Actions Context="Author"><Exec><Command>cmd.exe</Command><Arguments>{args}</Arguments></Exec></Actions>
</Task>
"#;

    pub(crate) fn escape_xml(s: &str) -> String {
        let mut out = String::with_capacity(s.len() + 16);
        for c in s.chars() {
            match c {
                '"' => out.push_str("&#34;"),
                '\'' => out.push_str("&#39;"),
                '&' => out.push_str("&amp;"),
                '<' => out.push_str("&lt;"),
                '>' => out.push_str("&gt;"),
                '\t' => out.push_str("&#x9;"),
                '\n' => out.push_str("&#xA;"),
                '\r' => out.push_str("&#xD;"),
                c => out.push(c),
            }
        }
        out
    }

    pub(crate) fn utf16le(s: &str) -> Vec<u8> {
        let mut out = vec![0xff, 0xfe];
        for u in s.encode_utf16() {
            out.extend_from_slice(&u.to_le_bytes());
        }
        out
    }

    pub(crate) fn command_line(state: &str, exe: &str, config: &str, log: &str) -> String {
        format!(
            r#"/c set "STATE_DIRECTORY={state}" && "{exe}" run --config "{config}" >> "{log}" 2>&1"#
        )
    }

    pub(crate) fn task_xml(start: &str, command: &str) -> String {
        TASK_XML
            .replace("{start}", start)
            .replace("{args}", &escape_xml(command))
    }

    pub(crate) fn needs_firewall(host: &str) -> bool {
        host != "127.0.0.1" && host != "localhost" && host != "::1"
    }
}

#[cfg(windows)]
mod windows {
    use std::path::{Path, PathBuf};

    use super::task::{TASK_NAME, command_line, needs_firewall, task_xml, utf16le};
    use super::{Runner, System, copy_file};
    use crate::cli::flags;
    use crate::cli::ui::{iso_local, unix_now};
    use crate::config::{self, Config};
    use crate::error::{Error, io_message};

    fn is_admin() -> bool {
        std::process::Command::new("net")
            .arg("session")
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .is_ok_and(|s| s.success())
    }

    fn dirs() -> (PathBuf, PathBuf) {
        let pf = std::env::var("ProgramFiles").unwrap_or_default();
        let pd = std::env::var("ProgramData").unwrap_or_default();
        (
            Path::new(&pf).join("gitsync"),
            Path::new(&pd).join("gitsync"),
        )
    }

    pub(in crate::cli) fn cmd_install(cfg_path: &str, args: &[String]) -> Result<i32, Error> {
        let mut no_start = None;
        let (path, fs) = flags("install", cfg_path, args, |fs| {
            no_start = Some(fs.bool("no-start", "create the task but do not start it"));
        })?;
        let no_start = no_start.is_some_and(|f| fs.on(f));
        if !is_admin() {
            return Err(Error::Config(
                "install needs an elevated prompt (Run as administrator)".into(),
            ));
        }
        let (bin, data) = dirs();
        let target = data.join("config.toml");
        let src = config::find(&path);
        let cfg = Config::load(&src)?;
        let self_exe = std::env::current_exe()?;
        let exe = bin.join("gitsync.exe");
        for d in [&bin, &data] {
            std::fs::create_dir_all(d)
                .map_err(|e| crate::err!("{}: {}", d.display(), io_message(&e)))?;
        }
        let sh = &mut System;
        let _ = sh.run("schtasks", &["/End", "/TN", TASK_NAME]);
        copy_file(&self_exe, &exe, 0o755)?;
        let abs = std::path::absolute(&src).unwrap_or_else(|_| PathBuf::from(&src));
        if abs != target {
            copy_file(Path::new(&src), &target, 0o600)?;
            if let Some(repos) = super::repos_beside(Path::new(&src), &cfg.paths.repos_file) {
                let dst = data.join("repos.toml");
                super::copy_repos(&repos, &dst, 0o600)?;
            }
        }
        let data_s = data.to_string_lossy();
        sh.run(
            "icacls",
            &[
                &data_s,
                "/inheritance:r",
                "/grant:r",
                "*S-1-5-18:(OI)(CI)F",
                "*S-1-5-32-544:(OI)(CI)F",
            ],
        )?;
        let rule = format!("name={TASK_NAME}");
        let _ = sh.run(
            "netsh",
            &["advfirewall", "firewall", "delete", "rule", &rule],
        );
        if needs_firewall(&cfg.listen.host) {
            let port = format!("localport={}", cfg.listen.port);
            sh.run(
                "netsh",
                &[
                    "advfirewall",
                    "firewall",
                    "add",
                    "rule",
                    &rule,
                    "dir=in",
                    "action=allow",
                    "protocol=TCP",
                    &port,
                ],
            )?;
            println!("firewall: allowed inbound TCP {}", cfg.listen.port);
        }
        let log = data.join("gitsync.log");
        let command = command_line(
            &data.join("state").to_string_lossy(),
            &exe.to_string_lossy(),
            &target.to_string_lossy(),
            &log.to_string_lossy(),
        );
        let tmp = std::env::temp_dir().join("gitsync-task.xml");
        std::fs::write(&tmp, utf16le(&task_xml(&iso_local(unix_now()), &command)))
            .map_err(|e| crate::err!("{}: {}", tmp.display(), io_message(&e)))?;
        let tmp_s = tmp.to_string_lossy().into_owned();
        let created = sh.run(
            "schtasks",
            &["/Create", "/TN", TASK_NAME, "/XML", &tmp_s, "/F"],
        );
        let _ = std::fs::remove_file(&tmp);
        created?;
        if no_start {
            println!("task created: schtasks /Run /TN gitsync");
            return Ok(0);
        }
        sh.run("schtasks", &["/Run", "/TN", TASK_NAME])?;
        println!(
            "task started, it also starts with Windows. Log: {}",
            log.display()
        );
        Ok(0)
    }

    pub(in crate::cli) fn cmd_uninstall(_: &str, _: &[String]) -> Result<i32, Error> {
        if !is_admin() {
            return Err(Error::Config(
                "uninstall needs an elevated prompt (Run as administrator)".into(),
            ));
        }
        let (bin, data) = dirs();
        let sh = &mut System;
        let _ = sh.run("schtasks", &["/End", "/TN", TASK_NAME]);
        let _ = sh.run("schtasks", &["/Delete", "/TN", TASK_NAME, "/F"]);
        let rule = format!("name={TASK_NAME}");
        let _ = sh.run(
            "netsh",
            &["advfirewall", "firewall", "delete", "rule", &rule],
        );
        let _ = std::fs::remove_file(bin.join("gitsync.exe"));
        println!(
            "removed the task and the binary; {} is kept",
            data.display()
        );
        Ok(0)
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::linux::{Layout, install, uninstall, unit};
    use super::task::{command_line, escape_xml, needs_firewall, task_xml, utf16le};
    use super::*;
    use crate::testutil::TempDir;

    #[derive(Default)]
    struct Fake(Vec<String>);

    impl Runner for Fake {
        fn run(&mut self, name: &str, args: &[&str]) -> Result<(), Error> {
            self.0.push(format!("{name} {}", args.join(" ")));
            Ok(())
        }
    }

    #[test]
    fn unit_is_the_hardened_one() {
        let u = unit(
            "forgejo",
            "/usr/local/bin/gitsync",
            "/etc/gitsync/config.toml",
        );
        assert!(u.starts_with("[Unit]\nDescription=Mirror Forgejo repositories to GitHub\n"));
        for line in [
            "User=gitsync",
            "ExecStart=/usr/local/bin/gitsync run --config /etc/gitsync/config.toml",
            "ExecReload=/bin/kill -HUP $MAINPID",
            "Restart=on-failure",
            "StateDirectory=gitsync",
            "ProtectSystem=strict",
            "ProtectHome=true",
            "PrivateTmp=true",
            "NoNewPrivileges=true",
            "MemoryMax=512M",
            "WantedBy=multi-user.target",
        ] {
            assert!(u.lines().any(|l| l == line), "missing {line}");
        }
    }

    #[test]
    fn install_and_uninstall_in_a_temp_root() {
        let dir = TempDir::new();
        let root = dir.path();
        let exe = root.join("built-gitsync");
        std::fs::write(&exe, b"binary").unwrap();
        let cfg = root.join("my.toml");
        std::fs::write(&cfg, b"[source]\n").unwrap();
        let l = Layout {
            bin: root.join("bin-gitsync"),
            etc: root.join("etc"),
            unit: root.join("gitsync.service"),
            bin_in_unit: "/usr/local/bin/gitsync".into(),
            config_in_unit: "/etc/gitsync/config.toml".into(),
        };
        let repos = root.join("repos.toml");
        std::fs::write(&repos, b"[repos]\n").unwrap();
        assert_eq!(repos_beside(&cfg, "elsewhere.toml"), None);
        assert_eq!(repos_beside(&cfg, "").as_deref(), Some(repos.as_path()));
        let mut run = Fake::default();
        assert_eq!(
            install(&l, &mut run, &exe, &cfg, Some(&repos), "gitea", true).unwrap(),
            0
        );
        assert_eq!(
            std::fs::read(l.etc.join("repos.toml")).unwrap(),
            b"[repos]\n"
        );
        assert_eq!(std::fs::read(&l.bin).unwrap(), b"binary");
        assert_eq!(
            std::fs::read(l.etc.join("config.toml")).unwrap(),
            b"[source]\n"
        );
        let text = std::fs::read_to_string(&l.unit).unwrap();
        assert!(text.contains("Mirror Gitea repositories"));
        assert_eq!(run.0.len(), 2);
        assert!(run.0[0].starts_with("chown root:gitsync "));
        assert!(run.0[0].ends_with("/etc/repos.toml"), "{}", run.0[0]);
        assert_eq!(run.0[1], "systemctl daemon-reload");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode =
                |p: &std::path::Path| std::fs::metadata(p).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode(&l.bin), 0o755);
            assert_eq!(mode(&l.etc), 0o750);
            assert_eq!(mode(&l.etc.join("config.toml")), 0o640);
            assert_eq!(mode(&l.etc.join("repos.toml")), 0o640);
        }

        std::fs::write(l.etc.join("repos.toml"), b"[repos]\nallow = [\"a\"]\n").unwrap();
        let mut run = Fake::default();
        install(&l, &mut run, &exe, &cfg, Some(&repos), "gitea", true).unwrap();
        assert_eq!(
            std::fs::read(l.etc.join("repos.toml")).unwrap(),
            b"[repos]\nallow = [\"a\"]\n"
        );

        let mut run = Fake::default();
        install(&l, &mut run, &exe, &cfg, None, "gitea", false).unwrap();
        assert_eq!(run.0.last().unwrap(), "systemctl enable --now gitsync");

        let mut run = Fake::default();
        uninstall(&l, &mut run);
        assert!(!l.unit.exists() && !l.bin.exists());
        assert!(l.etc.join("config.toml").exists());
        assert_eq!(
            run.0,
            vec!["systemctl disable --now gitsync", "systemctl daemon-reload"]
        );
    }

    #[test]
    fn windows_task_definition() {
        let cmd = command_line(
            r"C:\ProgramData\gitsync\state",
            r"C:\Program Files\gitsync\gitsync.exe",
            r"C:\ProgramData\gitsync\config.toml",
            r"C:\ProgramData\gitsync\gitsync.log",
        );
        assert_eq!(
            cmd,
            r#"/c set "STATE_DIRECTORY=C:\ProgramData\gitsync\state" && "C:\Program Files\gitsync\gitsync.exe" run --config "C:\ProgramData\gitsync\config.toml" >> "C:\ProgramData\gitsync\gitsync.log" 2>&1"#
        );
        let xml = task_xml("2026-11-13T10:00:00", &cmd);
        assert!(xml.contains("<Interval>PT1M</Interval>"));
        assert!(xml.contains("<MultipleInstancesPolicy>IgnoreNew</MultipleInstancesPolicy>"));
        assert!(xml.contains("<StartBoundary>2026-11-13T10:00:00</StartBoundary>"));
        assert!(
            xml.contains("&#34;STATE_DIRECTORY=C:\\ProgramData\\gitsync\\state&#34; &amp;&amp;")
        );
        assert!(
            xml.contains("&gt;&gt; &#34;C:\\ProgramData\\gitsync\\gitsync.log&#34; 2&gt;&amp;1")
        );
        assert_eq!(escape_xml("a'b\n"), "a&#39;b&#xA;");
        assert_eq!(utf16le("Añ"), vec![0xff, 0xfe, b'A', 0, 0xf1, 0]);
        assert!(!needs_firewall("127.0.0.1"));
        assert!(needs_firewall("0.0.0.0"));
    }
}
