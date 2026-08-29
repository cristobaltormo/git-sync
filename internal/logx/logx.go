package logx

import (
	"fmt"
	"os"
	"strings"
	"sync"
	"time"
)

const (
	Debug = iota
	Info
	Warn
	Error
	Off
)

var (
	mu      sync.Mutex
	level   = Info
	secrets []string
	journal = os.Getenv("JOURNAL_STREAM") != ""
)

func SetLevel(name string) {
	switch name {
	case "debug":
		level = Debug
	case "warn":
		level = Warn
	case "error":
		level = Error
	case "off":
		level = Off
	default:
		level = Info
	}
}

func Silence() { level = Off }

func Hide(s string) {
	if len(s) < 6 {
		return
	}
	mu.Lock()
	defer mu.Unlock()
	for _, h := range secrets {
		if h == s {
			return
		}
	}
	secrets = append(secrets, s)
}

func Clean(s string) string {
	mu.Lock()
	defer mu.Unlock()
	for _, h := range secrets {
		s = strings.ReplaceAll(s, h, "***")
	}
	return s
}

func write(lv int, tag, format string, args ...any) {
	if lv < level {
		return
	}
	msg := Clean(fmt.Sprintf(format, args...))
	stamp := ""
	if !journal {
		stamp = time.Now().Format("2006-01-02 15:04:05 ")
	}
	mu.Lock()
	fmt.Fprintf(os.Stderr, "%s%-5s %s\n", stamp, tag, msg)
	mu.Unlock()
}

func Debugf(f string, a ...any) { write(Debug, "debug", f, a...) }
func Infof(f string, a ...any)  { write(Info, "info", f, a...) }
func Warnf(f string, a ...any)  { write(Warn, "warn", f, a...) }
func Errorf(f string, a ...any) { write(Error, "error", f, a...) }
