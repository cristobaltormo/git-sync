package cli

import (
	"strings"
	"testing"

	"github.com/cristobaltormo/git-sync/internal/config"
	"github.com/cristobaltormo/git-sync/internal/engine"
	"github.com/cristobaltormo/git-sync/internal/forge"
)

func testModel(t *testing.T) *tuiModel {
	mk := func(id int64, name, status string, v engine.Verdict, managed bool) *item {
		return &item{Repo: &forge.Repo{ID: id, Owner: "alice", Name: name, Private: true}, Verdict: v, Status: status,
			Has: managed, Entry: engine.Entry{Managed: managed, GHOwner: "alice-gh", GHName: name}}
	}
	items := []*item{
		mk(1, "api", stSynced, engine.Verdict{Sync: true}, true),
		mk(2, "new-thing", stWaiting, engine.Verdict{Pending: true, Why: "waiting for your decision"}, false),
		mk(3, "old", stIgnored, engine.Verdict{Why: "ignored by you", Rule: &config.RepoRule{Sync: boolp(false)}}, false),
	}
	cfg := config.Default()
	cfg.Path = t.TempDir() + "/config.toml"
	m := newTUIModel(&app{cfg: cfg}, items)
	saved := 0
	m.save = func(m *tuiModel) error { saved++; return nil }
	return m
}

func typed(m *tuiModel, s string) {
	for _, k := range parseKeys([]byte(s)) {
		m.press(k)
	}
}

func TestParseKeys(t *testing.T) {
	ks := parseKeys([]byte("\x1b[A\x1b[B\x1b[5~\x1bj \r\x7f\x03\x1b"))
	want := []keyKind{kUp, kDown, kPgUp, kEsc, kRune, kRune, kEnter, kBackspace, kCtrlC, kEsc}
	if len(ks) != len(want) {
		t.Fatalf("got %d keys: %+v", len(ks), ks)
	}
	for i, k := range ks {
		if k.kind != want[i] {
			t.Errorf("key %d is %v, want %v", i, k.kind, want[i])
		}
	}
}

func TestTUIStartsOnWaitingAndDecides(t *testing.T) {
	m := testModel(t)
	eq := func(got, want any) {
		t.Helper()
		if got != want {
			t.Fatalf("got %v, want %v", got, want)
		}
	}
	eq(viewNames[m.view], "waiting")
	eq(len(m.visible()), 1)
	typed(m, " ")
	it := m.items[1]
	eq(m.desired(it), 1)
	eq(m.changes(), 1)
	typed(m, " ")
	eq(m.desired(it), 0)
	typed(m, "r")
	eq(m.changes(), 0)
}

func TestTUIBulkAndSave(t *testing.T) {
	m := testModel(t)
	typed(m, "fff")
	typed(m, "f")
	eq := func(got, want any) {
		t.Helper()
		if got != want {
			t.Fatalf("got %v, want %v", got, want)
		}
	}
	eq(viewNames[m.view], "waiting")
	typed(m, "A")
	eq(m.desired(m.items[1]), 1)
	typed(m, "w")
	eq(m.changes(), 0)
	if !strings.Contains(m.msg, "Saved") {
		t.Fatalf("no confirmation: %q", m.msg)
	}
}

func TestTUIQuitAsksWhenDirty(t *testing.T) {
	m := testModel(t)
	typed(m, " q")
	if m.quit || m.mode != mConfirm {
		t.Fatal("must ask before discarding")
	}
	typed(m, "n")
	if m.quit {
		t.Fatal("n must keep the session")
	}
	typed(m, "q")
	typed(m, "y")
	if !m.quit {
		t.Fatal("y must quit")
	}
}

func TestTUIOptionsAndValidation(t *testing.T) {
	m := testModel(t)
	typed(m, "o")
	typed(m, "\x1b[B")
	typed(m, "\r")
	typed(m, "bad name")
	typed(m, "\r")
	if m.changes() != 0 || !m.msgBad {
		t.Fatalf("an invalid GitHub name must be refused (changes %d, msg %q)", m.changes(), m.msg)
	}
	typed(m, "\r")
	typed(m, "\x7f\x7f\x7f\x7f\x7f\x7f\x7f\x7f")
	typed(m, "api-mirror\r")
	if r := m.rule(m.items[1]); r.Name != "api-mirror" {
		t.Fatalf("name not set: %+v", r)
	}
}

func TestTUISettingsStageAndProfile(t *testing.T) {
	m := testModel(t)
	typed(m, "\t")
	typed(m, "\r")
	if got := m.staged["filter.new_repos"]; got != `"review"` {
		t.Fatalf("staged %q", got)
	}
	typed(m, "G")
	typed(m, "\r")
	if m.profile != "personal" {
		t.Fatalf("first profile should be personal, got %q", m.profile)
	}
	typed(m, "\r")
	if m.profile != "team" || m.staged["sync.on_delete"] != `"archive"` || m.staged["filter.scope"] != `"admin"` {
		t.Fatalf("team profile not staged: %q %v", m.profile, m.staged)
	}
}

func TestTUIRendersWithinBounds(t *testing.T) {
	m := testModel(t)
	for _, size := range [][2]int{{100, 30}, {60, 12}, {30, 10}} {
		for _, key := range []string{"", "o", "\x1b", "\t", "?"} {
			typed(m, key)
			lines := m.render(size[0], size[1])
			if len(lines) != size[1] {
				t.Fatalf("%v after %q: %d lines", size, key, len(lines))
			}
			for _, l := range lines {
				if strings.Contains(l, "\n") {
					t.Fatal("a line contains a newline")
				}
			}
		}
		m.mode, m.screen = mList, scRepos
	}
}
