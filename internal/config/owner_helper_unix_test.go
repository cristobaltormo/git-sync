//go:build !windows

package config

import (
	"os"
	"syscall"
)

func owner(st os.FileInfo) (int, int) {
	s := st.Sys().(*syscall.Stat_t)
	return int(s.Uid), int(s.Gid)
}
