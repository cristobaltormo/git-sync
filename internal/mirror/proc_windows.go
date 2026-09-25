package mirror

import (
	"os/exec"
	"strconv"
	"syscall"
)

func groupAttr() *syscall.SysProcAttr { return &syscall.SysProcAttr{} }

func killTree(pid int) error {
	return exec.Command("taskkill", "/T", "/F", "/PID", strconv.Itoa(pid)).Run()
}
