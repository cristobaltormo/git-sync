//go:build !windows

package cli

func enableVT() bool { return true }
