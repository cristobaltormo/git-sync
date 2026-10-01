package cli

import (
	"bufio"
	"fmt"
	"os"
	"os/signal"
	"syscall"
	"unicode/utf8"

	"golang.org/x/term"

	"github.com/cristobaltormo/git-sync/internal/config"
)

func parseKeys(b []byte) []key {
	var out []key
	for len(b) > 0 {
		c := b[0]
		switch {
		case c == 0x1b:
			if len(b) >= 3 && (b[1] == '[' || b[1] == 'O') {
				k, n := parseEscape(b)
				if n > 0 {
					if k.kind != kRune || k.r != 0 {
						out = append(out, k)
					}
					b = b[n:]
					continue
				}
			}
			out = append(out, key{kind: kEsc})
			b = b[1:]
		case c == 3:
			out = append(out, key{kind: kCtrlC})
			b = b[1:]
		case c == '\r' || c == '\n':
			out = append(out, key{kind: kEnter})
			b = b[1:]
		case c == '\t':
			out = append(out, key{kind: kTab})
			b = b[1:]
		case c == 0x7f || c == 0x08:
			out = append(out, key{kind: kBackspace})
			b = b[1:]
		case c < 0x20:
			b = b[1:]
		default:
			r, n := utf8.DecodeRune(b)
			out = append(out, key{kind: kRune, r: r})
			b = b[n:]
		}
	}
	return out
}

func parseEscape(b []byte) (key, int) {
	switch b[2] {
	case 'A':
		return key{kind: kUp}, 3
	case 'B':
		return key{kind: kDown}, 3
	case 'C':
		return key{kind: kRight}, 3
	case 'D':
		return key{kind: kLeft}, 3
	case 'H':
		return key{kind: kHome}, 3
	case 'F':
		return key{kind: kEnd}, 3
	}
	if b[1] == '[' && b[2] >= '0' && b[2] <= '9' {
		end := 3
		for end < len(b) && (b[end] >= '0' && b[end] <= '9' || b[end] == ';') {
			end++
		}
		if end < len(b) {
			if b[end] == '~' {
				switch b[2] {
				case '1', '7':
					return key{kind: kHome}, end + 1
				case '4', '8':
					return key{kind: kEnd}, end + 1
				case '5':
					return key{kind: kPgUp}, end + 1
				case '6':
					return key{kind: kPgDn}, end + 1
				}
			}
			return key{}, end + 1
		}
	}
	return key{}, 0
}

func runTUI(m *tuiModel) error {
	in := int(os.Stdin.Fd())
	old, err := term.MakeRaw(in)
	if err != nil {
		return err
	}
	out := bufio.NewWriter(os.Stdout)
	restore := func() {
		fmt.Fprint(out, "\x1b[?25h\x1b[?1049l")
		out.Flush()
		term.Restore(in, old)
	}
	defer restore()
	fmt.Fprint(out, "\x1b[?1049h\x1b[?25l")

	keys := make(chan []byte, 4)
	go func() {
		buf := make([]byte, 128)
		for {
			n, err := os.Stdin.Read(buf)
			if n > 0 {
				keys <- append([]byte(nil), buf[:n]...)
			}
			if err != nil {
				close(keys)
				return
			}
		}
	}()
	stop := make(chan os.Signal, 1)
	signal.Notify(stop, syscall.SIGTERM, syscall.SIGHUP)
	defer signal.Stop(stop)
	resize, cleanup := resizeSignal()
	defer cleanup()

	for !m.quit {
		w, h := termSize()
		lines := m.render(w, h)
		for i := 0; i < h; i++ {
			text := ""
			if i < len(lines) {
				text = lines[i]
			}
			fmt.Fprintf(out, "\x1b[%d;1H%s\x1b[K", i+1, text)
		}
		out.Flush()
		select {
		case b, ok := <-keys:
			if !ok {
				return nil
			}
			for _, k := range parseKeys(b) {
				m.press(k)
				if m.quit {
					break
				}
			}
		case <-resize:
		case <-stop:
			return nil
		}
	}
	return nil
}

func cmdReposTUI(cfgPath string, args []string) (int, error) {
	a, _, err := openApp(cfgPath, args, nil)
	if err != nil {
		return 2, err
	}
	fmt.Fprint(os.Stderr, "Reading the repositories...")
	items, err := inventory(a)
	fmt.Fprint(os.Stderr, "\r\x1b[K")
	if err != nil {
		return 1, err
	}
	m := newTUIModel(a, items)
	m.refresh = func() error {
		cfg, err := config.Load(a.cfg.Path)
		if err != nil {
			return err
		}
		next, err := build(cfg)
		if err != nil {
			return err
		}
		fresh, err := inventory(next)
		if err != nil {
			return err
		}
		m.a, m.items = next, fresh
		*a = *next
		return nil
	}
	if err := runTUI(m); err != nil {
		return 1, err
	}
	return 0, nil
}
