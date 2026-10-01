//go:build windows

package cli

import "os"

func resizeSignal() (<-chan os.Signal, func()) { return nil, func() {} }
