//go:build windows

package config

import "os"

func keepOwner(string, os.FileInfo) {}
