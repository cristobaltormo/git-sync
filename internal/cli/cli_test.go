package cli

import (
	"bytes"
	"io"
	"os"
	"strings"
	"testing"
)

func capture(t *testing.T, f func() int) (int, string, string) {
	t.Helper()
	oldOut, oldErr := os.Stdout, os.Stderr
	ro, wo, _ := os.Pipe()
	re, we, _ := os.Pipe()
	os.Stdout, os.Stderr = wo, we
	code := f()
	wo.Close()
	we.Close()
	os.Stdout, os.Stderr = oldOut, oldErr
	var out, errb bytes.Buffer
	io.Copy(&out, ro)
	io.Copy(&errb, re)
	return code, out.String(), errb.String()
}

func TestVersion(t *testing.T) {
	code, out, _ := capture(t, func() int { return Run("1.2.3", []string{"version"}) })
	if code != 0 || strings.TrimSpace(out) != "gitsync 1.2.3" {
		t.Fatalf("got %d %q", code, out)
	}
}

func TestUnknownCommand(t *testing.T) {
	code, _, errText := capture(t, func() int { return Run("dev", []string{"frobnicate"}) })
	if code != 2 || !strings.Contains(errText, "unknown command") {
		t.Fatalf("got %d %q", code, errText)
	}
}

func TestNoCommandShowsUsage(t *testing.T) {
	code, _, errText := capture(t, func() int { return Run("dev", nil) })
	if code != 2 || !strings.Contains(errText, "Usage: gitsync") {
		t.Fatalf("got %d %q", code, errText)
	}
}

func TestMissingConfigPointsToInit(t *testing.T) {
	code, _, errText := capture(t, func() int { return Run("dev", []string{"-c", "/nonexistent/config.toml", "run"}) })
	if code != 2 || !strings.Contains(errText, "gitsync init") {
		t.Fatalf("got %d %q", code, errText)
	}
}
