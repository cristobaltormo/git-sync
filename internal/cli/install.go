package cli

import (
	"flag"
	"fmt"
	"io"
	"os"
	"os/exec"
	"os/user"
	"strings"

	"github.com/cristobaltormo/git-sync/internal/config"
)

const unit = `[Unit]
Description=Mirror %s repositories to GitHub
After=network-online.target
Wants=network-online.target

[Service]
User=gitsync
ExecStart=/usr/local/bin/gitsync run --config %s
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
`

func sh(name string, args ...string) error {
	out, err := exec.Command(name, args...).CombinedOutput()
	if err != nil {
		return fmt.Errorf("%s %s: %v: %s", name, strings.Join(args, " "), err, strings.TrimSpace(string(out)))
	}
	return nil
}

func copyFile(src, dst string, mode os.FileMode) error {
	in, err := os.Open(src)
	if err != nil {
		return err
	}
	defer in.Close()
	tmp := dst + ".new"
	out, err := os.OpenFile(tmp, os.O_WRONLY|os.O_CREATE|os.O_TRUNC, mode)
	if err != nil {
		return err
	}
	if _, err := io.Copy(out, in); err != nil {
		out.Close()
		return err
	}
	if err := out.Close(); err != nil {
		return err
	}
	return os.Rename(tmp, dst)
}

func cmdInstall(cfgPath string, args []string) (int, error) {
	var noStart *bool
	path, _, err := flags("install", cfgPath, args, func(fs *flag.FlagSet) {
		noStart = fs.Bool("no-start", false, "install the unit but do not start it")
	})
	if err != nil {
		return 2, err
	}
	if os.Geteuid() != 0 {
		return 2, config.Errorf("install needs root (try: sudo gitsync install)")
	}
	const target = "/etc/gitsync/config.toml"
	src := config.Find(path)
	cfg, err := config.Load(src)
	if err != nil {
		return 2, err
	}
	if _, err := user.Lookup("gitsync"); err != nil {
		if err := sh("useradd", "--system", "--home-dir", "/var/lib/gitsync", "--no-create-home",
			"--shell", "/usr/sbin/nologin", "gitsync"); err != nil {
			return 1, err
		}
		fmt.Println("created system user gitsync")
	}
	self, err := os.Executable()
	if err != nil {
		return 1, err
	}
	if err := copyFile(self, "/usr/local/bin/gitsync", 0o755); err != nil {
		return 1, err
	}
	if err := os.MkdirAll("/etc/gitsync", 0o750); err != nil {
		return 1, err
	}
	if src != target {
		if err := copyFile(src, target, 0o640); err != nil {
			return 1, err
		}
	}
	if err := sh("chown", "root:gitsync", "/etc/gitsync", target); err != nil {
		return 1, err
	}
	os.Chmod("/etc/gitsync", 0o750)
	os.Chmod(target, 0o640)
	name := cfg.Source.Type
	content := fmt.Sprintf(unit, strings.ToUpper(name[:1])+name[1:], target)
	if err := os.WriteFile("/etc/systemd/system/gitsync.service", []byte(content), 0o644); err != nil {
		return 1, err
	}
	if err := sh("systemctl", "daemon-reload"); err != nil {
		return 1, err
	}
	if *noStart {
		fmt.Println("unit installed: systemctl enable --now gitsync")
		return 0, nil
	}
	if err := sh("systemctl", "enable", "--now", "gitsync"); err != nil {
		return 1, err
	}
	fmt.Println("service started: journalctl -u gitsync -f")
	return 0, nil
}

func cmdUninstall(cfgPath string, args []string) (int, error) {
	if os.Geteuid() != 0 {
		return 2, config.Errorf("uninstall needs root")
	}
	sh("systemctl", "disable", "--now", "gitsync")
	os.Remove("/etc/systemd/system/gitsync.service")
	os.Remove("/usr/local/bin/gitsync")
	sh("systemctl", "daemon-reload")
	fmt.Println("removed the service and the binary; /etc/gitsync and /var/lib/gitsync are kept")
	return 0, nil
}
