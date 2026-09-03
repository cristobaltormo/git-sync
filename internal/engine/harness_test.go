package engine

import (
	"fmt"
	"sort"
	"strings"
	"sync"
	"testing"

	"github.com/cristobaltormo/git-sync/internal/config"
	"github.com/cristobaltormo/git-sync/internal/forge"
	"github.com/cristobaltormo/git-sync/internal/github"
	"github.com/cristobaltormo/git-sync/internal/httpx"
	"github.com/cristobaltormo/git-sync/internal/logx"
	"github.com/cristobaltormo/git-sync/internal/mirror"
)

var (
	events   []string
	eventsMu sync.Mutex
)

func addEvent(e string) {
	eventsMu.Lock()
	events = append(events, e)
	eventsMu.Unlock()
}

type fakeSource struct {
	repos   map[int64]*forge.Repo
	ownerOK bool
	listErr error
	hooks   forge.Hooks
	admin   bool
}

func newFakeSource() *fakeSource { return &fakeSource{repos: map[int64]*forge.Repo{}, ownerOK: true} }

func (f *fakeSource) Kind() string { return "fake" }

func (f *fakeSource) Whoami() (*forge.Identity, error) {
	return &forge.Identity{Login: "alice", Admin: f.admin, SystemHooks: f.admin}, nil
}

func (f *fakeSource) set(r *forge.Repo) { f.repos[r.ID] = r }

func (f *fakeSource) List(owner string) ([]*forge.Repo, error) {
	if f.listErr != nil {
		return nil, f.listErr
	}
	var out []*forge.Repo
	for _, r := range f.repos {
		if r.Owner == owner {
			c := *r
			out = append(out, &c)
		}
	}
	return out, nil
}

func (f *fakeSource) Get(owner, name string) (*forge.Repo, error) {
	for _, r := range f.repos {
		if r.Owner == owner && r.Name == name {
			c := *r
			return &c, nil
		}
	}
	return nil, nil
}

func (f *fakeSource) OwnerExists(string) (bool, error) { return f.ownerOK, nil }

func (f *fakeSource) Hooks() forge.Hooks { return f.hooks }

type fakeHooks struct {
	global, repo []string
}

func (h *fakeHooks) Global(known int64, force bool) (int64, string, error) {
	h.global = append(h.global, "global")
	return 42, "created", nil
}

func (h *fakeHooks) Repo(r *forge.Repo, force bool) (string, error) {
	h.repo = append(h.repo, r.Full())
	return "created", nil
}

func (h *fakeHooks) RemoveGlobal(int64) bool { return true }

func (h *fakeHooks) RemoveRepo(*forge.Repo) int { return 1 }

type fakeTarget struct {
	refuseBranch bool
	mu           sync.Mutex
	repos        map[string]*github.Repo
	calls        []string
}

func newFakeTarget() *fakeTarget { return &fakeTarget{repos: map[string]*github.Repo{}} }

func ptr(s string) *string { return &s }

func (g *fakeTarget) Get(owner, name string) (*github.Repo, error) {
	g.mu.Lock()
	defer g.mu.Unlock()
	r := g.repos[owner+"/"+name]
	if r == nil {
		return nil, nil
	}
	c := *r
	c.Topics = append([]string(nil), r.Topics...)
	return &c, nil
}

func (g *fakeTarget) Create(owner, name, desc, home string) (*github.Repo, error) {
	g.mu.Lock()
	defer g.mu.Unlock()
	g.calls = append(g.calls, "create "+owner+"/"+name)
	addEvent("create")
	r := &github.Repo{Name: name, Private: true, Description: ptr(desc), Homepage: ptr(home), DefaultBranch: "main"}
	r.Owner.Login = owner
	g.repos[owner+"/"+name] = r
	c := *r
	return &c, nil
}

func (g *fakeTarget) Patch(owner, name string, f map[string]any) error {
	g.mu.Lock()
	defer g.mu.Unlock()
	g.calls = append(g.calls, "patch "+owner+"/"+name+" "+fields(f))
	if _, ok := f["default_branch"]; ok && g.refuseBranch {
		return &httpx.APIError{Status: 422, Msg: "branch not found"}
	}
	addEvent("patch " + fields(f))
	key := owner + "/" + name
	r := g.repos[key]
	if v, ok := f["name"]; ok {
		delete(g.repos, key)
		r.Name = v.(string)
		g.repos[owner+"/"+r.Name] = r
	}
	if v, ok := f["private"]; ok {
		r.Private = v.(bool)
	}
	if v, ok := f["archived"]; ok {
		r.Archived = v.(bool)
	}
	if v, ok := f["description"]; ok {
		r.Description = ptr(v.(string))
	}
	if v, ok := f["homepage"]; ok {
		r.Homepage = ptr(v.(string))
	}
	if v, ok := f["default_branch"]; ok {
		r.DefaultBranch = v.(string)
	}
	return nil
}

func (g *fakeTarget) SetTopics(owner, name string, t []string) error {
	g.mu.Lock()
	defer g.mu.Unlock()
	g.calls = append(g.calls, "topics "+owner+"/"+name)
	g.repos[owner+"/"+name].Topics = t
	return nil
}

func (g *fakeTarget) Delete(owner, name string) error {
	g.mu.Lock()
	defer g.mu.Unlock()
	g.calls = append(g.calls, "delete "+owner+"/"+name)
	delete(g.repos, owner+"/"+name)
	return nil
}

func (g *fakeTarget) has(key string) bool { return g.repos[key] != nil }

type fakeMirror struct {
	mu    sync.Mutex
	calls []string
	lasts []string
	fail  error
}

func (m *fakeMirror) Mirror(r *forge.Repo, o, n, last string) (mirror.Result, error) {
	m.mu.Lock()
	defer m.mu.Unlock()
	if m.fail != nil {
		return mirror.Result{}, m.fail
	}
	m.calls = append(m.calls, "mirror "+r.Full()+" "+o+"/"+n)
	m.lasts = append(m.lasts, last)
	addEvent("mirror")
	return mirror.Result{Summary: "1 ref(s) updated", Refs: "refs-" + r.Updated}, nil
}

func (m *fakeMirror) Forget(id int64) { m.calls = append(m.calls, fmt.Sprintf("forget %d", id)) }

type harness struct {
	t   *testing.T
	src *fakeSource
	tgt *fakeTarget
	mir *fakeMirror
	e   *Engine
}

func testConfig(t *testing.T, mod func(*config.Config)) *config.Config {
	c := config.Default()
	c.Source.URL, c.Source.Token = "https://git.example.com", "src-token-123456"
	c.GitHub.Token = "gh-token-123456"
	c.Accounts = map[string]string{"alice": "alice-gh", "my-org": "my-org-gh"}
	c.Listen.Secret = "s3cret-value-123"
	c.Paths.StateDir = t.TempDir()
	c.Sync.DeleteGrace = 0
	c.Log.Level = "off"
	if mod != nil {
		mod(c)
	}
	if err := c.Finish(); err != nil {
		t.Fatal(err)
	}
	return c
}

func newHarness(t *testing.T, mod func(*config.Config)) *harness {
	logx.Silence()
	events = nil
	h := &harness{t: t, src: newFakeSource(), tgt: newFakeTarget(), mir: &fakeMirror{}}
	h.rebuild(mod)
	return h
}

func (h *harness) rebuild(mod func(*config.Config)) {
	dir := ""
	if h.e != nil {
		dir = h.e.Config().Paths.StateDir
	}
	cfg := testConfig(h.t, func(c *config.Config) {
		if dir != "" {
			c.Paths.StateDir = dir
		}
		if mod != nil {
			mod(c)
		}
	})
	e, err := New(cfg, h.src, h.tgt, h.mir)
	if err != nil {
		h.t.Fatal(err)
	}
	h.e = e
}

func mk(id int64, name string, mod ...func(*forge.Repo)) *forge.Repo {
	r := &forge.Repo{ID: id, Name: name, Owner: "alice", Private: true, DefaultBranch: "main",
		Updated: "2026-01-01T00:00:00Z"}
	for _, m := range mod {
		m(r)
	}
	return r
}

func upd(ts string) func(*forge.Repo) { return func(r *forge.Repo) { r.Updated = ts } }

func (h *harness) pump() {
	q := &h.e.queue
	for {
		q.mu.Lock()
		if len(q.order) == 0 {
			q.mu.Unlock()
			return
		}
		k := q.order[0]
		q.order = q.order[1:]
		job := q.pending[k]
		delete(q.pending, k)
		q.mu.Unlock()
		h.e.execute(job)
	}
}

func (h *harness) run(verify bool) {
	h.e.Reconcile(verify, "scan")
	h.pump()
}

func eq(t *testing.T, got, want any) {
	t.Helper()
	if fmt.Sprint(got) != fmt.Sprint(want) {
		t.Fatalf("got %v, want %v", got, want)
	}
}

func isTrue(t *testing.T, v bool, msg string) {
	t.Helper()
	if !v {
		t.Fatal(msg)
	}
}

func synced(t *testing.T, mod func(*config.Config)) *harness {
	h := newHarness(t, mod)
	h.src.set(mk(1, "site"))
	h.run(false)
	h.tgt.calls = nil
	return h
}

func fields(f map[string]any) string {
	parts := make([]string, 0, len(f))
	for k, v := range f {
		parts = append(parts, fmt.Sprintf("%s=%v", k, v))
	}
	sort.Strings(parts)
	return strings.Join(parts, ", ")
}
