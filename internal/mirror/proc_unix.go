//go:build !windows

package mirror

import "syscall"

func groupAttr() *syscall.SysProcAttr { return &syscall.SysProcAttr{Setpgid: true} }

func killTree(pid int) error { return syscall.Kill(-pid, syscall.SIGKILL) }
