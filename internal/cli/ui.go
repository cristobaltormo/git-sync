package cli

import (
	"fmt"
	"os"
	"strings"
	"unicode/utf8"

	"golang.org/x/term"
)

type style string

const (
	reset  style = "\x1b[0m"
	bold   style = "\x1b[1m"
	dim    style = "\x1b[2m"
	red    style = "\x1b[31m"
	green  style = "\x1b[32m"
	yellow style = "\x1b[33m"
	blue   style = "\x1b[34m"
	cyan   style = "\x1b[36m"
	invert style = "\x1b[7m"
)

var colorOn = detectColor()

func detectColor() bool {
	if os.Getenv("NO_COLOR") != "" || os.Getenv("TERM") == "dumb" {
		return false
	}
	if !term.IsTerminal(int(os.Stdout.Fd())) {
		return false
	}
	return enableVT()
}

func paint(s string, st ...style) string {
	if !colorOn || len(st) == 0 {
		return s
	}
	var b strings.Builder
	for _, x := range st {
		b.WriteString(string(x))
	}
	b.WriteString(s)
	b.WriteString(string(reset))
	return b.String()
}

func width(s string) int { return utf8.RuneCountInString(s) }

func pad(s string, n int) string {
	if w := width(s); w < n {
		return s + strings.Repeat(" ", n-w)
	}
	return s
}

func clip(s string, n int) string {
	if n <= 0 {
		return ""
	}
	r := []rune(s)
	if len(r) <= n {
		return s
	}
	if n == 1 {
		return string(r[:1])
	}
	return string(r[:n-1]) + "~"
}

func termSize() (int, int) {
	w, h, err := term.GetSize(int(os.Stdout.Fd()))
	if err != nil || w < 20 {
		return 100, 30
	}
	return w, h
}

type table struct {
	head []string
	rows [][]string
	sty  [][]style
}

func (t *table) add(cells []string, st ...style) {
	t.rows = append(t.rows, cells)
	t.sty = append(t.sty, st)
}

func (t *table) print() {
	cols := make([]int, len(t.head))
	for i, h := range t.head {
		cols[i] = width(h)
	}
	for _, r := range t.rows {
		for i, c := range r {
			cols[i] = max(cols[i], width(c))
		}
	}
	line := func(cells []string, st style, per []style) {
		var b strings.Builder
		b.WriteString(" ")
		for i, c := range cells {
			cell := pad(c, cols[i])
			if i == len(cells)-1 {
				cell = c
			}
			s := st
			if per != nil && i < len(per) && per[i] != "" {
				s = per[i]
			}
			if s != "" {
				cell = paint(cell, s)
			}
			b.WriteString(cell)
			b.WriteString("  ")
		}
		fmt.Println(strings.TrimRight(b.String(), " "))
	}
	line(t.head, dim, nil)
	for i, r := range t.rows {
		line(r, "", t.sty[i])
	}
}

func heading(s string) { fmt.Println(paint(s, bold)) }
