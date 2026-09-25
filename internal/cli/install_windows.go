//go:build windows

package cli

import (
	"bytes"
	"encoding/xml"
	"flag"
	"fmt"
	"os"
	"os/exec"
	"path/filepath"
	"time"

	"github.com/cristobaltormo/git-sync/internal/config"
)

const taskName = "gitsync"

const taskXML = `<?xml version="1.0" encoding="UTF-16"?>
<Task version="1.2" xmlns="http://schemas.microsoft.com/windows/2004/02/mit/task">
  <RegistrationInfo><Description>Mirror git repositories to GitHub</Description></RegistrationInfo>
  <Triggers>
    <BootTrigger><Enabled>true</Enabled></BootTrigger>
    <TimeTrigger>
      <Repetition><Interval>PT1M</Interval><StopAtDurationEnd>false</StopAtDurationEnd></Repetition>
      <StartBoundary>%s</StartBoundary>
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
  <Actions Context="Author"><Exec><Command>cmd.exe</Command><Arguments>%s</Arguments></Exec></Actions>
</Task>
`

func isAdmin() bool { return exec.Command("net", "session").Run() == nil }

func dirs() (bin, data string) {
	return filepath.Join(os.Getenv("ProgramFiles"), "gitsync"), filepath.Join(os.Getenv("ProgramData"), "gitsync")
}

func utf16le(s string) []byte {
	out := []byte{0xff, 0xfe}
	for _, r := range s {
		out = append(out, byte(r), byte(r>>8))
	}
	return out
}

func cmdInstall(cfgPath string, args []string) (int, error) {
	var noStart *bool
	path, _, err := flags("install", cfgPath, args, func(fs *flag.FlagSet) {
		noStart = fs.Bool("no-start", false, "create the task but do not start it")
	})
	if err != nil {
		return 2, err
	}
	if !isAdmin() {
		return 2, config.Errorf("install needs an elevated prompt (Run as administrator)")
	}
	bin, data := dirs()
	target := filepath.Join(data, "config.toml")
	src := config.Find(path)
	cfg, err := config.Load(src)
	if err != nil {
		return 2, err
	}
	self, err := os.Executable()
	if err != nil {
		return 1, err
	}
	exe := filepath.Join(bin, "gitsync.exe")
	for _, d := range []string{bin, data} {
		if err := os.MkdirAll(d, 0o755); err != nil {
			return 1, err
		}
	}
	sh("schtasks", "/End", "/TN", taskName)
	if err := copyFile(self, exe, 0o755); err != nil {
		return 1, err
	}
	if abs, _ := filepath.Abs(src); abs != target {
		if err := copyFile(src, target, 0o600); err != nil {
			return 1, err
		}
	}
	if err := sh("icacls", data, "/inheritance:r", "/grant:r", "*S-1-5-18:(OI)(CI)F", "*S-1-5-32-544:(OI)(CI)F"); err != nil {
		return 1, err
	}
	sh("netsh", "advfirewall", "firewall", "delete", "rule", "name="+taskName)
	if h := cfg.Listen.Host; h != "127.0.0.1" && h != "localhost" && h != "::1" {
		if err := sh("netsh", "advfirewall", "firewall", "add", "rule", "name="+taskName, "dir=in", "action=allow",
			"protocol=TCP", fmt.Sprintf("localport=%d", cfg.Listen.Port)); err != nil {
			return 1, err
		}
		fmt.Printf("firewall: allowed inbound TCP %d\n", cfg.Listen.Port)
	}
	var arg bytes.Buffer
	xml.EscapeText(&arg, []byte(fmt.Sprintf(`/c set "STATE_DIRECTORY=%s" && "%s" run --config "%s" >> "%s" 2>&1`, filepath.Join(data, "state"), exe, target, filepath.Join(data, "gitsync.log"))))
	tmp := filepath.Join(os.TempDir(), "gitsync-task.xml")
	if err := os.WriteFile(tmp, utf16le(fmt.Sprintf(taskXML, time.Now().Format("2006-01-02T15:04:05"), arg.String())), 0o600); err != nil {
		return 1, err
	}
	defer os.Remove(tmp)
	if err := sh("schtasks", "/Create", "/TN", taskName, "/XML", tmp, "/F"); err != nil {
		return 1, err
	}
	if *noStart {
		fmt.Println("task created: schtasks /Run /TN gitsync")
		return 0, nil
	}
	if err := sh("schtasks", "/Run", "/TN", taskName); err != nil {
		return 1, err
	}
	fmt.Printf("task started, it also starts with Windows. Log: %s\n", filepath.Join(data, "gitsync.log"))
	return 0, nil
}

func cmdUninstall(cfgPath string, args []string) (int, error) {
	if !isAdmin() {
		return 2, config.Errorf("uninstall needs an elevated prompt (Run as administrator)")
	}
	bin, data := dirs()
	sh("schtasks", "/End", "/TN", taskName)
	sh("schtasks", "/Delete", "/TN", taskName, "/F")
	sh("netsh", "advfirewall", "firewall", "delete", "rule", "name="+taskName)
	os.Remove(filepath.Join(bin, "gitsync.exe"))
	fmt.Printf("removed the task and the binary; %s is kept\n", data)
	return 0, nil
}
