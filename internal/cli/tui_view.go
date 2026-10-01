package cli

import (
	"fmt"
	"strings"
	"time"
)

func (m *tuiModel) render(w, h int) []string {
	m.width, m.height = w, h
	var lines []string
	lines = append(lines, m.header(w)...)
	body := h - 3 - 3
	var rows []string
	switch {
	case m.mode == mHelp:
		rows = helpLines()
	case m.mode == mOptions || (m.mode == mInput && m.screen == scRepos):
		rows = m.optionsView(w)
	case m.screen == scSettings:
		rows = m.settingsView(w, body)
	default:
		rows = m.listView(w, body)
	}
	for len(rows) < body {
		rows = append(rows, "")
	}
	lines = append(lines, rows[:body]...)
	lines = append(lines, m.footer(w)...)
	return lines
}

func (m *tuiModel) header(w int) []string {
	repos, sets := " Repositories ", " Settings "
	if m.screen == scRepos {
		repos = paint(repos, invert)
	} else {
		sets = paint(sets, invert)
	}
	title := paint(" gitsync ", bold) + " " + repos + sets
	if n := m.changes(); n > 0 {
		title += "  " + paint(fmt.Sprintf("%d unsaved", n), yellow, bold)
	}
	var info string
	if m.screen == scRepos {
		c := counts(m.items)
		info = fmt.Sprintf(" %d repositories: %d synced, %d waiting, %d skipped",
			len(m.items), c[stSynced]+c[stError]+c[stQueued], c[stWaiting], c[stIgnored]+c[stFiltered])
		info = paint(info, dim) + "   view: " + paint(viewNames[m.view], bold)
		if m.mode == mSearch || m.query != "" {
			cur := ""
			if m.mode == mSearch {
				cur = "_"
			}
			info += "   search: " + paint(m.query+cur, bold)
		}
	} else {
		info = paint(" How gitsync behaves. Everything here can be changed at any time; nothing is applied until you save.", dim)
	}
	return []string{title, clipStyled(info, w), paint(strings.Repeat("-", w), dim)}
}

func clipStyled(s string, w int) string {
	if !strings.Contains(s, "\x1b") {
		return clip(s, w)
	}
	return s
}

func (m *tuiModel) listView(w, body int) []string {
	vis := m.visible()
	if len(vis) == 0 {
		msg := " Nothing to show here."
		if m.query != "" {
			msg = " No repository matches the search."
		} else if viewNames[m.view] == "waiting" {
			msg = " Nothing is waiting for a decision. Press f to see the other views."
		}
		return []string{"", paint(msg, dim)}
	}
	m.cursor = max(0, min(m.cursor, len(vis)-1))
	if m.cursor < m.top {
		m.top = m.cursor
	}
	if m.cursor >= m.top+body {
		m.top = m.cursor - body + 1
	}
	nameW := 12
	for _, it := range vis {
		nameW = max(nameW, width(it.full()))
	}
	nameW = min(nameW, max(w/3, 18))
	var out []string
	for i := m.top; i < len(vis) && i < m.top+body; i++ {
		it := vis[i]
		box := "[ ]"
		switch m.desired(it) {
		case 1:
			box = "[x]"
		case -1:
			box = "[~]"
		}
		status := it.Status
		mark := " "
		if m.dirty(it) {
			mark = "*"
		}
		rest := w - 2 - 4 - nameW - 2 - 10 - 2
		extra := note(it)
		if it.Status == stWaiting {
			extra = "new, waiting for your decision"
		}
		if t := it.target(); t != "" && it.Status != stError && it.Status != stWaiting {
			extra = t
			if n := note(it); n != "" && (it.Status == stIgnored || it.Status == stFiltered) {
				extra = n
			}
		}
		plain := fmt.Sprintf("%s %s %s  %s %s  %s", cursorMark(i == m.cursor), box, pad(clip(it.full(), nameW), nameW),
			pad(status, 9), mark, clip(extra, max(rest, 0)))
		if i == m.cursor {
			out = append(out, paint(pad(clip(plain, w), w), invert))
			continue
		}
		cells := fmt.Sprintf("%s %s %s  ", cursorMark(false), box, pad(clip(it.full(), nameW), nameW))
		line := cells + paint(pad(status, 9), statusStyle(status)) + " " + paint(mark, yellow, bold) + "  " +
			paint(clip(extra, max(rest, 0)), dim)
		out = append(out, line)
	}
	return out
}

func cursorMark(on bool) string {
	if on {
		return ">"
	}
	return " "
}

func (m *tuiModel) optionsView(w int) []string {
	it := m.current()
	if it == nil {
		return nil
	}
	out := []string{paint(" Options for "+it.full(), bold), ""}
	for i, r := range m.options(it) {
		line := fmt.Sprintf(" %s %s  %s", cursorMark(i == m.opt), pad(r.label, 30), r.value)
		if i == m.opt {
			out = append(out, paint(pad(clip(line, w), w), invert))
		} else {
			out = append(out, line)
		}
	}
	out = append(out, "", paint(" The options override the general settings for this repository only.", dim))
	return out
}

func (m *tuiModel) settingsView(w, body int) []string {
	n := len(settingRows) + 1
	m.cursor = max(0, min(m.cursor, n-1))
	if m.cursor < m.top {
		m.top = m.cursor
	}
	if m.cursor >= m.top+body {
		m.top = m.cursor - body + 1
	}
	var out []string
	for i := m.top; i < n && i < m.top+body; i++ {
		label, value, staged := "Apply a ready-made profile", "choose with Enter", false
		if i < len(settingRows) {
			s := settingRows[i]
			label, value = s.label, m.settingValue(s)
			_, staged = m.staged[s.name]
			if s.text && value == "" {
				value = "(none)"
			}
		} else if m.profile != "" {
			value = m.profile + " (staged)"
		}
		mark := " "
		if staged {
			mark = "*"
		}
		line := fmt.Sprintf(" %s %s %s %s", cursorMark(i == m.cursor), pad(label, 30), pad(value, 22), mark)
		if i == m.cursor {
			out = append(out, paint(pad(clip(line, w), w), invert))
		} else if staged {
			out = append(out, paint(clip(line, w), yellow))
		} else {
			out = append(out, clip(line, w))
		}
	}
	return out
}

func helpLines() []string {
	return []string{
		paint(" Keys", bold),
		"",
		"  up/down, j/k      move          PgUp/PgDn, g/G   jump",
		"  space             toggle this repository (sync or ignore)",
		"  a / i / r         sync / ignore / back to the general rules",
		"  A / I             sync / ignore every repository shown now",
		"  f                 change the view: all, waiting, synced, skipped",
		"  /                 search by name (Esc clears it)",
		"  o or Enter        options for this repository: GitHub name, keep private, tags...",
		"  Tab               switch between Repositories and Settings",
		"  w                 write the changes (a running gitsync applies them on its own)",
		"  q                 quit (asks first if there are unsaved changes)",
		"",
		paint("  [x] will sync   [ ] will not   [~] the general rules decide   * unsaved", dim),
		"",
		paint(" Press any key.", dim),
	}
}

func (m *tuiModel) footer(w int) []string {
	detail := ""
	switch {
	case m.screen == scSettings && m.cursor < len(settingRows):
		detail = " " + settingRows[m.cursor].help
	case m.screen == scSettings:
		detail = " personal: sync everything. team: new repos wait for you. careful: only what you pick."
	case m.mode == mList || m.mode == mSearch:
		if it := m.current(); it != nil {
			detail = " " + detailLine(it)
		}
	case m.mode == mOptions:
		detail = " Enter or space changes the highlighted option, left goes back, Esc closes."
	}
	msg := ""
	switch {
	case m.mode == mInput:
		msg = paint(" "+m.msg+": ", bold) + m.input + "_"
	case m.mode == mConfirm:
		msg = paint(fmt.Sprintf(" Discard %d unsaved change(s)? y/n", m.changes()), yellow, bold)
	case m.msg != "" && m.msgBad:
		msg = paint(" "+m.msg, red)
	case m.msg != "":
		msg = paint(" "+m.msg, green)
	}
	keys := " space toggle  a sync  i ignore  r reset  f view  / search  o options  w save  Tab settings  ? help  q quit"
	switch {
	case m.screen == scSettings:
		keys = " Enter/space change  left back  w save  Tab repositories  ? help  q quit"
	case m.mode == mOptions:
		keys = " up/down choose  Enter change  left back  Esc close"
	case m.mode == mSearch:
		keys = " type to search  Enter keep  Esc clear"
	case m.mode == mInput:
		keys = " Enter accept  Esc cancel"
	}
	return []string{paint(clip(detail, w), dim), msg, paint(clip(keys, w), invert)}
}

func detailLine(it *item) string {
	parts := []string{}
	if it.Verdict.Why != "" {
		parts = append(parts, it.Verdict.Why)
	}
	vis := "public"
	if it.Repo.Private {
		vis = "private"
	}
	parts = append(parts, vis)
	if it.Repo.Fork {
		parts = append(parts, "fork")
	}
	if it.Repo.Role != "" {
		parts = append(parts, "you: "+it.Repo.Role)
	}
	if it.Has && it.Entry.LastOK > 0 {
		parts = append(parts, "synced "+ago(time.Now().Unix()-it.Entry.LastOK))
	}
	if it.Entry.LastError != "" {
		parts = append(parts, "error: "+it.Entry.LastError)
	}
	return strings.Join(parts, " | ")
}
