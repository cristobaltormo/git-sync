//go:build windows

package config

import "os"

func owner(os.FileInfo) (int, int) { return 0, 0 }
