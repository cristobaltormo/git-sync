package forge

import (
	"crypto/hmac"
	"crypto/sha256"
	"encoding/base64"
	"encoding/hex"
	"encoding/json"
	"fmt"
	"io"
	"net/http"
	"net/http/httptest"
	"strings"
	"sync"
	"testing"
	"time"

	"github.com/cristobaltormo/git-sync/internal/config"
	"github.com/cristobaltormo/git-sync/internal/logx"
)

type fakeGitea struct {
	mu       sync.Mutex
	repos    []map[string]any
	hooks    []map[string]any
	repoHook []map[string]any
	requests []string
	auth     string
	plain    bool
}

func (g *fakeGitea) ServeHTTP(w http.ResponseWriter, r *http.Request) {
	g.mu.Lock()
	defer g.mu.Unlock()
	g.requests = append(g.requests, r.Method+" "+r.URL.RequestURI())
	g.auth = r.Header.Get("Authorization")
	out := func(v any) { json.NewEncoder(w).Encode(v) }
	p := r.URL.Path
	idOf := func(prefix string) string { return strings.TrimPrefix(p, prefix) }
	switch {
	case p == "/api/v1/user":
		out(map[string]any{"login": "alice", "is_admin": true})
	case p == "/api/v1/users/alice" || p == "/api/v1/users/acme":
		out(map[string]any{"id": map[string]int{"alice": 1, "acme": 2}[idOf("/api/v1/users/")]})
	case p == "/api/v1/repos/search" && !g.plain:
		page, limit := atoi(r.URL.Query().Get("page"), 1), atoi(r.URL.Query().Get("limit"), 30)
		uid := r.URL.Query().Get("uid")
		var mine []map[string]any
		for _, d := range g.repos {
			owner := d["owner"].(map[string]any)["login"].(string)
			if (uid == "1" && (owner == "alice" || owner == "stale")) || (uid == "2" && owner == "acme") {
				mine = append(mine, d)
			}
		}
		lo, hi := min((page-1)*limit, len(mine)), min(page*limit, len(mine))
		out(map[string]any{"ok": true, "data": mine[lo:hi]})
	case p == "/api/v1/users/alice/repos" && g.plain:
		if atoi(r.URL.Query().Get("page"), 1) > 1 {
			out([]any{})
			return
		}
		out(g.repos)
	case p == "/api/v1/repos/alice/site":
		out(g.repos[0])
	case p == "/api/v1/admin/hooks" && r.Method == "GET":
		out([]any{}) // like Forgejo: system webhooks are never listed
	case strings.HasPrefix(p, "/api/v1/admin/hooks/") && r.Method == "GET":
		for _, h := range g.hooks {
			if fmt.Sprint(h["id"]) == idOf("/api/v1/admin/hooks/") {
				out(h)
				return
			}
		}
		http.NotFound(w, r)
	case p == "/api/v1/admin/hooks" && r.Method == "POST":
		var body map[string]any
		json.NewDecoder(r.Body).Decode(&body)
		body["id"] = len(g.hooks) + 100
		g.hooks = append(g.hooks, body)
		w.WriteHeader(201)
		out(body)
	case strings.HasPrefix(p, "/api/v1/admin/hooks/") && r.Method == "PATCH":
		io.Copy(io.Discard, r.Body)
		out(map[string]any{})
	case strings.HasPrefix(p, "/api/v1/admin/hooks/") && r.Method == "DELETE":
		var keep []map[string]any
		for _, h := range g.hooks {
			if fmt.Sprint(h["id"]) != idOf("/api/v1/admin/hooks/") {
				keep = append(keep, h)
			}
		}
		g.hooks = keep
		w.WriteHeader(204)
	case p == "/api/v1/repos/alice/site/hooks" && r.Method == "GET":
		out(g.repoHook)
	case p == "/api/v1/repos/alice/site/hooks" && r.Method == "POST":
		var body map[string]any
		json.NewDecoder(r.Body).Decode(&body)
		body["id"] = len(g.repoHook) + 200
		g.repoHook = append(g.repoHook, body)
		w.WriteHeader(201)
		out(body)
	default:
		http.NotFound(w, r)
	}
}

func atoi(s string, def int) int {
	var n int
	if _, err := fmt.Sscan(s, &n); err != nil || n == 0 {
		return def
	}
	return n
}

func repoJSON(id int, owner, name string) map[string]any {
	return map[string]any{"id": id, "name": name, "full_name": owner + "/" + name,
		"owner": map[string]any{"login": owner}, "private": true, "updated_at": "2026-01-01T00:00:00Z"}
}

func newTestGitea(t *testing.T, typ string, g *fakeGitea) *Gitea {
	logx.Silence()
	srv := httptest.NewServer(g)
	t.Cleanup(srv.Close)
	cfg := config.Default()
	cfg.Source.Type, cfg.Source.URL, cfg.Source.Token = typ, srv.URL, "src-token-123456"
	cfg.Listen.Secret = "s3cret-value-123"
	p := newGitea(cfg)
	p.http.SetSleep(func(time.Duration) {})
	return p
}

func TestGiteaListPaginatesAndFiltersOwner(t *testing.T) {
	g := &fakeGitea{}
	for i := 1; i <= 120; i++ {
		g.repos = append(g.repos, repoJSON(i, "alice", fmt.Sprintf("r%d", i)))
	}
	g.repos = append(g.repos, repoJSON(999, "stale", "moved"))
	p := newTestGitea(t, "forgejo", g)
	repos, err := p.List("alice")
	if err != nil {
		t.Fatal(err)
	}
	if len(repos) != 120 {
		t.Fatalf("got %d repos", len(repos))
	}
	if g.auth != "token src-token-123456" {
		t.Fatalf("auth header: %s", g.auth)
	}
	pages := 0
	for _, r := range g.requests {
		if strings.Contains(r, "/repos/search") {
			pages++
		}
	}
	if pages != 3 {
		t.Fatalf("expected 3 pages, got %d", pages)
	}
}

func TestGiteaGet(t *testing.T) {
	g := &fakeGitea{repos: []map[string]any{repoJSON(1, "alice", "site")}}
	p := newTestGitea(t, "gitea", g)
	if r, err := p.Get("alice", "site"); err != nil || r == nil || r.Full() != "alice/site" {
		t.Fatalf("got %v %v", r, err)
	}
	if r, err := p.Get("alice", "missing"); r != nil || err != nil {
		t.Fatalf("a 404 must be (nil, nil): %v %v", r, err)
	}
}

func TestGogsListing(t *testing.T) {
	g := &fakeGitea{plain: true, repos: []map[string]any{repoJSON(1, "alice", "site")}}
	repos, err := newTestGitea(t, "gogs", g).List("alice")
	if err != nil || len(repos) != 1 {
		t.Fatalf("got %v %v", repos, err)
	}
}

func TestGiteaHooks(t *testing.T) {
	g := &fakeGitea{repos: []map[string]any{repoJSON(1, "alice", "site")}}
	h := newTestGitea(t, "forgejo", g).Hooks()
	id, res, err := h.Global(0, false)
	if err != nil || res != "created" {
		t.Fatalf("%v %v", res, err)
	}
	cfg := g.hooks[0]["config"].(map[string]any)
	if cfg["url"] != "http://127.0.0.1:9001/hook" || cfg["secret"] != "s3cret-value-123" || g.hooks[0]["type"] != "gitea" {
		t.Fatalf("hook body: %v", g.hooks[0])
	}
	if fmt.Sprint(g.hooks[0]["events"]) != "[push create delete repository]" {
		t.Fatalf("events: %v", g.hooks[0]["events"])
	}
	if id2, res, _ := h.Global(id, false); res != "exists" || id2 != id {
		t.Fatalf("looking it up by id must find it: %v %v", id2, res)
	}
	if _, res, _ := h.Global(id, true); res != "updated" {
		t.Fatalf("force must rewrite it: %v", res)
	}
	id3, res, _ := h.Global(0, false)
	if res != "created" || id3 == id || len(g.hooks) != 1 {
		t.Fatalf("a lost id must create a new hook and drop the old copy: %v %v %d", id3, res, len(g.hooks))
	}
	if !h.RemoveGlobal(id3) || len(g.hooks) != 0 {
		t.Fatal("removing the known hook")
	}
	repo := &Repo{Owner: "alice", Name: "site"}
	if res, _ := h.Repo(repo, false); res != "created" {
		t.Fatalf("repo hook: %v", res)
	}
	if res, _ := h.Repo(repo, false); res != "exists" {
		t.Fatalf("repo hook again: %v", res)
	}
}

func TestOnlyForgejoUsesSystemWebhooksByDefault(t *testing.T) {
	for typ, want := range map[string]bool{"forgejo": true, "codeberg": true, "gitea": false} {
		id, err := newTestGitea(t, typ, &fakeGitea{}).Whoami()
		if err != nil || !id.Admin || id.SystemHooks != want {
			t.Errorf("%s: %+v %v", typ, id, err)
		}
	}
}

func TestGitHeaders(t *testing.T) {
	g := &fakeGitea{}
	if h, _ := newTestGitea(t, "forgejo", g).GitHeader(); h != "Authorization: token src-token-123456" {
		t.Fatalf("forgejo: %s", h)
	}
	h, err := newTestGitea(t, "gogs", g).GitHeader()
	want := "Authorization: Basic " + base64.StdEncoding.EncodeToString([]byte("alice:src-token-123456"))
	if err != nil || h != want {
		t.Fatalf("gogs wants basic auth as the token owner: %s %v", h, err)
	}
}

func TestRejectedToken(t *testing.T) {
	srv := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) { w.WriteHeader(401) }))
	defer srv.Close()
	cfg := config.Default()
	cfg.Source.URL, cfg.Source.Token = srv.URL, "src-token-123456"
	_, err := newGitea(cfg).Whoami()
	if err == nil || !strings.Contains(err.Error(), "rejected") {
		t.Fatalf("got %v", err)
	}
}

func mac(secret, body string) string {
	m := hmac.New(sha256.New, []byte(secret))
	m.Write([]byte(body))
	return hex.EncodeToString(m.Sum(nil))
}

func TestGiteaWebhookParsing(t *testing.T) {
	p := newTestGitea(t, "forgejo", &fakeGitea{})
	body := `{"action":"deleted","repository":{"id":7,"full_name":"alice/site"}}`
	sig := mac("s3cret", body)
	for _, name := range []string{"X-Gitea-Signature", "X-Forgejo-Signature", "X-Gogs-Signature"} {
		ev, err := p.ParseWebhook(http.Header{name: {sig}, "X-Gitea-Event": {"push"}}, []byte(body), "s3cret")
		if err != nil || ev == nil || ev.Owner != "alice" || ev.Name != "site" || ev.ID != 7 || ev.Kind != Changed {
			t.Errorf("%s: %+v %v", name, ev, err)
		}
	}
	if ev, err := p.ParseWebhook(http.Header{"X-Hub-Signature-256": {"sha256=" + sig}, "X-Gogs-Event": {"push"}}, []byte(body), "s3cret"); err != nil || ev == nil {
		t.Errorf("github style signature: %v %v", ev, err)
	}
	ev, _ := p.ParseWebhook(http.Header{"X-Gitea-Signature": {sig}, "X-Gitea-Event": {"repository"}}, []byte(body), "s3cret")
	if ev == nil || ev.Kind != Removed {
		t.Errorf("a deleted repository event must be Removed: %+v", ev)
	}
	if ev, _ := p.ParseWebhook(http.Header{"X-Gitea-Signature": {sig}, "X-Gitea-Event": {"issues"}}, []byte(body), "s3cret"); ev != nil {
		t.Errorf("unrelated events are ignored: %+v", ev)
	}
	for name, h := range map[string]http.Header{
		"missing": {},
		"wrong":   {"X-Gitea-Signature": {strings.Repeat("0", 64)}},
		"other":   {"X-Gitea-Signature": {mac("other", body)}},
	} {
		if _, err := p.ParseWebhook(h, []byte(body), "s3cret"); err != ErrSignature {
			t.Errorf("%s signature must be rejected: %v", name, err)
		}
	}
}
