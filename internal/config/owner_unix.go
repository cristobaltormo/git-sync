//go:build !windows

package config

import (
	"os"
	"syscall"
)

func keepOwner(path string, st os.FileInfo) {
	if s, ok := st.Sys().(*syscall.Stat_t); ok {
		os.Chown(path, int(s.Uid), int(s.Gid))
	}
}
