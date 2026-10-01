package cli

import (
	"fmt"
	"reflect"
	"strconv"
	"strings"

	"github.com/cristobaltormo/git-sync/internal/config"
)

type keyKind int

const (
	kRune keyKind = iota
	kUp
	kDown
	kLeft
	kRight
	kPgUp
	kPgDn
	kHome
	kEnd
	kEnter
	kEsc
	kTab
	kBackspace
	kCtrlC
)

type key struct {
	kind keyKind
	r    rune
}

type setting struct {
	name    string
	label   string
	help    string
	choices []string
	text    bool
}

var settingRows = []setting{
	{name: "filter.new_repos", label: "New repositories", help: "sync: start right away. review: wait for your OK. ignore: only what you pick.", choices: []string{"sync", "review", "ignore"}},
	{name: "filter.scope", label: "Which repos count", help: "all, or only those where you are an administrator (Forgejo, Gitea, Gogs).", choices: []string{"all", "admin"}},
	{name: "filter.skip_forks", label: "Skip forks", choices: []string{"false", "true"}},
	{name: "filter.skip_private", label: "Skip private repos", choices: []string{"false", "true"}},
	{name: "filter.skip_archived", label: "Skip archived repos", choices: []string{"false", "true"}},
	{name: "filter.skip_mirrors", label: "Skip pull mirrors", choices: []string{"true", "false"}},
	{name: "sync.visibility", label: "Follow visibility", help: "Public on the source becomes public on GitHub. Private never becomes public.", choices: []string{"true", "false"}},
	{name: "sync.tags", label: "Mirror tags", choices: []string{"true", "false"}},
	{name: "sync.metadata", label: "Copy description and topics", choices: []string{"true", "false"}},
	{name: "sync.on_delete", label: "When a repo is deleted", help: "What happens to the GitHub copy.", choices: []string{"delete", "archive", "ignore"}},
	{name: "pull_requests.mode", label: "Pull requests on mirrors", help: "leave: do nothing. comment: add one note. They are never merged or closed.", choices: []string{"leave", "comment"}},
	{name: "notify.url", label: "Notification webhook", help: "Receives a message when a repo is waiting, fails or gets a pull request.", text: true},
	{name: "sync.poll_interval", label: "Poll interval (seconds)", help: "How often the source is checked. 0 relies on webhooks only.", text: true},
}

type screen int

const (
	scRepos screen = iota
	scSettings
)

type mode int

const (
	mList mode = iota
	mSearch
	mOptions
	mInput
	mConfirm
	mHelp
)

var viewNames = []string{"all", "waiting", "synced", "skipped"}

type tuiModel struct {
	a       *app
	items   []*item
	orig    map[string]config.RepoRule
	draft   map[string]*config.RepoRule
	staged  map[string]string
	screen  screen
	mode    mode
	view    int
	query   string
	cursor  int
	top     int
	opt     int
	input   string
	inputFn func(string)
	confirm func()
	msg     string
	msgBad  bool
	quit    bool
	save    func(m *tuiModel) error
	profile string
	refresh func() error
	height  int
	width   int
}

func newTUIModel(a *app, items []*item) *tuiModel {
	m := &tuiModel{a: a, items: items, orig: map[string]config.RepoRule{}, draft: map[string]*config.RepoRule{},
		staged: map[string]string{}, width: 100, height: 30}
	for _, it := range items {
		if it.Verdict.Rule != nil {
			m.orig[lower(it.full())] = *it.Verdict.Rule
		}
	}
	if n := m.pendingCount(); n > 0 {
		m.view = 1
		m.say("%d new repositor%s waiting: space picks, A syncs all shown, I ignores all shown, w saves", n,
			map[bool]string{true: "y is", false: "ies are"}[n == 1])
	}
	m.save = saveTUI
	return m
}

func lower(s string) string { return strings.ToLower(s) }

func (m *tuiModel) rule(it *item) config.RepoRule {
	if d, ok := m.draft[lower(it.full())]; ok {
		return *d
	}
	return m.orig[lower(it.full())]
}

func (m *tuiModel) edit(it *item, fn func(*config.RepoRule)) {
	k := lower(it.full())
	d := m.draft[k]
	if d == nil {
		c := m.orig[k]
		d = &c
		m.draft[k] = d
	}
	fn(d)
	if reflect.DeepEqual(*d, m.orig[k]) {
		delete(m.draft, k)
	}
}

func (m *tuiModel) dirty(it *item) bool {
	_, ok := m.draft[lower(it.full())]
	return ok
}

func (m *tuiModel) changes() int { return len(m.draft) + len(m.staged) }

func (m *tuiModel) pendingCount() int {
	n := 0
	for _, it := range m.items {
		if it.Status == stWaiting {
			n++
		}
	}
	return n
}

// 1 = will sync, 0 = will not, -1 = decided by the general rules
func (m *tuiModel) desired(it *item) int {
	r := m.rule(it)
	if r.Sync != nil {
		if *r.Sync {
			return 1
		}
		return 0
	}
	if m.dirty(it) || (it.Verdict.Rule != nil && it.Verdict.Rule.Sync != nil) {
		return -1
	}
	if it.Verdict.Sync {
		return 1
	}
	return 0
}

func (m *tuiModel) visible() []*item {
	var out []*item
	q := lower(m.query)
	for _, it := range m.items {
		if q != "" && !strings.Contains(lower(it.full()), q) {
			continue
		}
		switch viewNames[m.view] {
		case "waiting":
			if it.Status != stWaiting {
				continue
			}
		case "synced":
			if it.Status != stSynced && it.Status != stError && it.Status != stQueued {
				continue
			}
		case "skipped":
			if it.Status != stIgnored && it.Status != stFiltered {
				continue
			}
		}
		out = append(out, it)
	}
	return out
}

func (m *tuiModel) current() *item {
	v := m.visible()
	if len(v) == 0 {
		return nil
	}
	m.cursor = max(0, min(m.cursor, len(v)-1))
	return v[m.cursor]
}

func (m *tuiModel) setSync(it *item, v *bool) {
	m.edit(it, func(r *config.RepoRule) { r.Sync = v })
}

func boolp(b bool) *bool { return &b }

func (m *tuiModel) say(format string, a ...any) {
	m.msg, m.msgBad = fmt.Sprintf(format, a...), false
}

func (m *tuiModel) warn(format string, a ...any) {
	m.msg, m.msgBad = fmt.Sprintf(format, a...), true
}

func (m *tuiModel) press(k key) {
	m.msg = ""
	switch m.mode {
	case mSearch:
		m.pressSearch(k)
	case mOptions:
		m.pressOptions(k)
	case mInput:
		m.pressInput(k)
	case mConfirm:
		if k.kind == kRune && (k.r == 'y' || k.r == 'Y') {
			fn := m.confirm
			m.mode, m.confirm = mList, nil
			fn()
		} else {
			m.mode, m.confirm = mList, nil
		}
	case mHelp:
		m.mode = mList
	default:
		if m.screen == scSettings {
			m.pressSettings(k)
		} else {
			m.pressList(k)
		}
	}
}

func (m *tuiModel) tryQuit() {
	if m.changes() == 0 {
		m.quit = true
		return
	}
	m.mode = mConfirm
	m.confirm = func() { m.quit = true }
}

func (m *tuiModel) move(k key, n int) bool {
	page := max(m.height-9, 3)
	switch k.kind {
	case kUp:
		m.cursor--
	case kDown:
		m.cursor++
	case kPgUp:
		m.cursor -= page
	case kPgDn:
		m.cursor += page
	case kHome:
		m.cursor = 0
	case kEnd:
		m.cursor = n - 1
	case kRune:
		switch k.r {
		case 'k':
			m.cursor--
		case 'j':
			m.cursor++
		case 'g':
			m.cursor = 0
		case 'G':
			m.cursor = n - 1
		default:
			return false
		}
	default:
		return false
	}
	m.cursor = max(0, min(m.cursor, max(n-1, 0)))
	return true
}

func (m *tuiModel) pressList(k key) {
	vis := m.visible()
	if m.move(k, len(vis)) {
		return
	}
	it := m.current()
	switch k.kind {
	case kCtrlC, kEsc:
		if k.kind == kEsc && m.query != "" {
			m.query = ""
			return
		}
		m.tryQuit()
	case kTab:
		m.screen = scSettings
		m.cursor = 0
	case kEnter:
		if it != nil {
			m.mode, m.opt = mOptions, 0
		}
	case kRune:
		m.pressListRune(k.r, it, vis)
	}
}

func (m *tuiModel) pressListRune(r rune, it *item, vis []*item) {
	switch r {
	case 'q':
		m.tryQuit()
	case ' ':
		if it == nil {
			return
		}
		m.setSync(it, boolp(m.desired(it) != 1))
		if m.cursor < len(vis)-1 && viewNames[m.view] == "all" {
			m.cursor++
		}
	case 'a', 'A':
		if r == 'A' {
			for _, x := range vis {
				m.setSync(x, boolp(true))
			}
			m.say("%d repositories will be synced", len(vis))
		} else if it != nil {
			m.setSync(it, boolp(true))
		}
	case 'i', 'I':
		if r == 'I' {
			for _, x := range vis {
				m.setSync(x, boolp(false))
			}
			m.say("%d repositories will be ignored", len(vis))
		} else if it != nil {
			m.setSync(it, boolp(false))
		}
	case 'r':
		if it != nil {
			m.setSync(it, nil)
		}
	case 'f':
		m.view = (m.view + 1) % len(viewNames)
		m.cursor, m.top = 0, 0
	case '/':
		m.mode = mSearch
	case 'o':
		if it != nil {
			m.mode, m.opt = mOptions, 0
		}
	case 'w', 's':
		m.doSave()
	case '?':
		m.mode = mHelp
	}
}

func (m *tuiModel) pressSearch(k key) {
	switch k.kind {
	case kEsc:
		m.query, m.mode = "", mList
	case kEnter, kUp, kDown:
		m.mode = mList
	case kBackspace:
		if r := []rune(m.query); len(r) > 0 {
			m.query = string(r[:len(r)-1])
		}
	case kCtrlC:
		m.mode = mList
	case kRune:
		m.query += string(k.r)
	}
	m.cursor, m.top = 0, 0
}

type optionRow struct {
	label string
	value string
}

func (m *tuiModel) options(it *item) []optionRow {
	r := m.rule(it)
	tri := func(p *bool) string {
		switch {
		case p == nil:
			return "default"
		case *p:
			return "yes"
		}
		return "no"
	}
	name := r.Name
	if name == "" {
		name = "same as the source"
	}
	od := r.OnDelete
	if od == "" {
		od = "default"
	}
	note := r.Note
	if note == "" {
		note = "-"
	}
	return []optionRow{
		{"Sync this repository", map[int]string{1: "yes", 0: "no", -1: "as the general rules say"}[m.desired(it)]},
		{"Name on GitHub", name},
		{"Keep the copy private", tri(r.KeepPrivate)},
		{"Mirror tags", tri(r.Tags)},
		{"Copy description and topics", tri(r.Metadata)},
		{"When removed from the source", od},
		{"Note", note},
	}
}

func cycleBool(p *bool, back bool) *bool {
	steps := []*bool{nil, boolp(true), boolp(false)}
	i := 0
	for n, s := range steps {
		if (s == nil && p == nil) || (s != nil && p != nil && *s == *p) {
			i = n
		}
	}
	if back {
		i += len(steps) - 1
	} else {
		i++
	}
	return steps[i%len(steps)]
}

func (m *tuiModel) pressOptions(k key) {
	it := m.current()
	if it == nil {
		m.mode = mList
		return
	}
	rows := m.options(it)
	switch k.kind {
	case kEsc, kCtrlC:
		m.mode = mList
		return
	case kUp:
		m.opt--
	case kDown, kTab:
		m.opt++
	case kRune:
		switch k.r {
		case 'q':
			m.mode = mList
			return
		case 'k':
			m.opt--
		case 'j':
			m.opt++
		case ' ':
			m.changeOption(it, false)
		}
	case kEnter, kRight:
		m.changeOption(it, false)
	case kLeft:
		m.changeOption(it, true)
	}
	m.opt = (m.opt + len(rows)) % len(rows)
}

func (m *tuiModel) changeOption(it *item, back bool) {
	switch m.opt {
	case 0:
		d := m.desired(it)
		next := map[int]*bool{1: boolp(false), 0: nil, -1: boolp(true)}[d]
		if back {
			next = map[int]*bool{1: nil, 0: boolp(true), -1: boolp(false)}[d]
		}
		m.setSync(it, next)
	case 1:
		cur := m.rule(it).Name
		m.ask("GitHub name (empty: same as the source)", cur, func(v string) {
			m.apply(it, func(r *config.RepoRule) { r.Name = v })
		})
	case 2:
		m.edit(it, func(r *config.RepoRule) { r.KeepPrivate = cycleBool(r.KeepPrivate, back) })
	case 3:
		m.edit(it, func(r *config.RepoRule) { r.Tags = cycleBool(r.Tags, back) })
	case 4:
		m.edit(it, func(r *config.RepoRule) { r.Metadata = cycleBool(r.Metadata, back) })
	case 5:
		cs := []string{"", "delete", "archive", "ignore"}
		m.edit(it, func(r *config.RepoRule) { r.OnDelete = cycleChoice(cs, r.OnDelete, back) })
	case 6:
		m.ask("Note", m.rule(it).Note, func(v string) {
			m.apply(it, func(r *config.RepoRule) { r.Note = v })
		})
	}
}

func (m *tuiModel) apply(it *item, fn func(*config.RepoRule)) {
	probe := m.rule(it)
	fn(&probe)
	if err := validateRule(it.full(), probe); err != nil {
		m.warn("%v", strings.TrimPrefix(err.Error(), "repos."+strconv.Quote(it.full())+": "))
		return
	}
	m.edit(it, fn)
}

func validateRule(full string, r config.RepoRule) error {
	probe := &config.Repos{Rules: map[string]*config.RepoRule{}}
	return probe.Edit(full, func(x *config.RepoRule) { *x = r })
}

func cycleChoice(choices []string, cur string, back bool) string {
	i := 0
	for n, c := range choices {
		if c == cur {
			i = n
		}
	}
	if back {
		i += len(choices) - 1
	} else {
		i++
	}
	return choices[i%len(choices)]
}

func (m *tuiModel) ask(prompt, def string, fn func(string)) {
	m.mode, m.input, m.inputFn = mInput, def, fn
	m.msg = prompt
}

func (m *tuiModel) pressInput(k key) {
	switch k.kind {
	case kEsc, kCtrlC:
		m.mode, m.inputFn = m.returnMode(), nil
	case kEnter:
		fn, v := m.inputFn, strings.TrimSpace(m.input)
		m.mode, m.inputFn = m.returnMode(), nil
		if fn != nil {
			fn(v)
		}
	case kBackspace:
		if r := []rune(m.input); len(r) > 0 {
			m.input = string(r[:len(r)-1])
		}
	case kRune:
		m.input += string(k.r)
	}
}

func (m *tuiModel) returnMode() mode {
	if m.screen == scSettings {
		return mList
	}
	return mOptions
}

func (m *tuiModel) settingValue(s setting) string {
	if lit, ok := m.staged[s.name]; ok {
		return strings.Trim(lit, `"`)
	}
	k, _ := config.FindKey(s.name)
	return k.Get(m.a.cfg)
}

func (m *tuiModel) pressSettings(k key) {
	n := len(settingRows) + 1
	if m.move(k, n) {
		return
	}
	switch k.kind {
	case kCtrlC, kEsc:
		m.tryQuit()
		return
	case kTab:
		m.screen, m.cursor = scRepos, 0
		return
	case kLeft:
		m.changeSetting(true)
	case kEnter, kRight:
		m.changeSetting(false)
	case kRune:
		switch k.r {
		case 'q':
			m.tryQuit()
		case ' ':
			m.changeSetting(false)
		case 'w', 's':
			m.doSave()
		case '?':
			m.mode = mHelp
		}
	}
}

func (m *tuiModel) stage(name, lit string) {
	k, _ := config.FindKey(name)
	if lit == m.literalOf(k) {
		delete(m.staged, name)
		return
	}
	m.staged[name] = lit
}

func (m *tuiModel) literalOf(k config.Key) string {
	lit, err := k.Literal([]string{k.Get(m.a.cfg)})
	if err != nil {
		return ""
	}
	return lit
}

func (m *tuiModel) changeSetting(back bool) {
	if m.cursor == len(settingRows) {
		names := []string{}
		for _, p := range config.Profiles {
			names = append(names, p.Name)
		}
		m.profile = cycleChoice(append([]string{""}, names...), m.profile, back)
		if p, ok := config.FindProfile(m.profile); ok {
			for k, v := range p.Set {
				m.stage(k, v)
			}
			m.say("Profile %s staged: review the settings above, then save", p.Name)
		}
		return
	}
	s := settingRows[m.cursor]
	cur := m.settingValue(s)
	if s.text {
		m.ask(s.label, cur, func(v string) {
			key, _ := config.FindKey(s.name)
			lit, err := key.Literal([]string{v})
			if err != nil {
				m.warn("%v", err)
				return
			}
			if v != "" && s.name == "notify.url" && !config.IsHTTPURL(v) {
				m.warn("that is not an http(s) URL")
				return
			}
			m.stage(s.name, lit)
		})
		return
	}
	next := cycleChoice(s.choices, cur, back)
	key, _ := config.FindKey(s.name)
	lit, _ := key.Literal([]string{next})
	m.stage(s.name, lit)
}

func (m *tuiModel) doSave() {
	if m.changes() == 0 {
		m.say("Nothing to save")
		return
	}
	n := m.changes()
	if err := m.save(m); err != nil {
		m.warn("%v", err)
		return
	}
	if m.refresh != nil {
		if err := m.refresh(); err != nil {
			m.warn("saved, but could not refresh the list: %v", err)
			m.afterSave()
			return
		}
		m.draft, m.staged = map[string]*config.RepoRule{}, map[string]string{}
		m.orig = map[string]config.RepoRule{}
		for _, it := range m.items {
			if it.Verdict.Rule != nil {
				m.orig[lower(it.full())] = *it.Verdict.Rule
			}
		}
	} else {
		m.afterSave()
	}
	m.say("Saved %d change(s). A running gitsync applies them on its own.", n)
}

func saveTUI(m *tuiModel) error {
	if len(m.staged) > 0 {
		if err := config.SetValues(m.a.cfg.Path, m.staged); err != nil {
			return err
		}
	}
	if len(m.draft) > 0 {
		err := saveRepos(m.a, func(r *config.Repos) error {
			for k, d := range m.draft {
				d := *d
				if err := r.Edit(k, func(x *config.RepoRule) { *x = d }); err != nil {
					return err
				}
			}
			return nil
		})
		if err != nil {
			return err
		}
	}
	return nil
}

func (m *tuiModel) afterSave() {
	for _, it := range m.items {
		k := lower(it.full())
		if d, ok := m.draft[k]; ok {
			m.orig[k] = *d
			if reflect.DeepEqual(*d, config.RepoRule{}) {
				delete(m.orig, k)
			}
			c := *d
			if reflect.DeepEqual(c, config.RepoRule{}) {
				it.Verdict.Rule = nil
			} else {
				it.Verdict.Rule = &c
			}
			switch {
			case c.Sync != nil && *c.Sync:
				it.Status = stQueued
				if it.managed() {
					it.Status = stSynced
				}
				it.Verdict.Sync, it.Verdict.Pending = true, false
			case c.Sync != nil:
				it.Status, it.Verdict.Sync, it.Verdict.Pending, it.Verdict.Why = stIgnored, false, false, "ignored by you"
			}
		}
	}
	m.draft = map[string]*config.RepoRule{}
	m.staged = map[string]string{}
}
