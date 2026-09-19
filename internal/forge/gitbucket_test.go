package forge

import (
	"encoding/base64"
	"encoding/json"
	"net/http"
	"strings"
	"testing"
	"time"
)

func TestGitBucket(t *testing.T) {
	var hooks []map[string]any
	cfg, srv := serve(t, "gitbucket", func(w http.ResponseWriter, r *http.Request, body []byte) {
		p := r.URL.Path
		switch {
		case p == "/api/v3/user":
			js(w, map[string]any{"login": "root", "site_admin": true})
		case p == "/api/v3/users/root/repos":
			js(w, []any{
				map[string]any{"id": 1, "name": "r1", "full_name": "root/r1", "private": true, "description": "hola",
					"default_branch": "main", "fork": false, "archived": nil, "updated_at": nil, "homepage": nil,
					"owner": map[string]any{"login": "root"}},
				map[string]any{"id": 2, "name": "x", "full_name": "other/x", "private": false,
					"owner": map[string]any{"login": "other"}},
			})
		case p == "/api/v3/users/root":
			js(w, map[string]any{"login": "root"})
		case p == "/api/v3/repos/root/r1":
			js(w, map[string]any{"id": 1, "name": "r1", "full_name": "root/r1", "private": false, "owner": map[string]any{"login": "root"}})
		case p == "/api/v3/repos/root/gone":
			http.NotFound(w, r)
		case p == "/api/v3/repos/root/r1/hooks" && r.Method == "GET":
			js(w, hooks)
		case p == "/api/v3/repos/root/r1/hooks" && r.Method == "POST":
			var h map[string]any
			json.Unmarshal(body, &h)
			h["id"] = 7
			hooks = append(hooks, h)
			js(w, h)
		case p == "/api/v3/repos/root/r1/hooks/7" && r.Method == "DELETE":
			hooks = nil
			w.WriteHeader(204)
		default:
			http.NotFound(w, r)
		}
	})
	g := newGitBucket(cfg)
	g.http.SetSleep(func(time.Duration) {})
	repos, err := g.List("root")
	if err != nil || len(repos) != 1 {
		t.Fatalf("list must keep only the owner's repos: %v %v", repos, err)
	}
	r := repos[0]
	if !r.Private || r.Description != "hola" || r.Updated != "" || r.Website != "" {
		t.Fatalf("null fields must be empty: %+v", r)
	}
	if srv.auth != "token src-token-123456" {
		t.Fatalf("auth: %s", srv.auth)
	}
	if got, _ := g.Get("root", "r1"); got == nil || got.Private {
		t.Fatalf("get: %+v", got)
	}
	if got, _ := g.Get("root", "gone"); got != nil {
		t.Fatal("404")
	}
	if u := g.CloneURL(r); !strings.HasSuffix(u, "/git/root/r1.git") {
		t.Fatalf("clone url: %s", u)
	}
	if h, err := g.GitHeader(); err != nil || h != "Authorization: Basic "+base64.StdEncoding.EncodeToString([]byte("root:src-token-123456")) {
		t.Fatalf("git header: %s %v", h, err)
	}
	h := g.Hooks()
	if _, _, err := h.Global(0, false); err == nil {
		t.Fatal("GitBucket has no system webhooks")
	}
	repo := &Repo{Owner: "root", Name: "r1"}
	if res, _ := h.Repo(repo, false); res != "created" {
		t.Fatalf("hook: %s", res)
	}
	hooks[0]["config"] = map[string]any{"url": cfg.HookURL()}
	if res, _ := h.Repo(repo, false); res != "exists" {
		t.Fatalf("hook again: %s", res)
	}
	if h.RemoveRepo(repo) != 1 {
		t.Fatal("remove")
	}
}

func TestGitBucketWebhooks(t *testing.T) {
	cfg, _ := serve(t, "gitbucket", func(http.ResponseWriter, *http.Request, []byte) {})
	g := newGitBucket(cfg)
	body := `{"ref":"refs/heads/main","repository":{"id":1,"full_name":"root/r1"}}`
	sig := http.Header{"X-Hub-Signature-256": {"sha256=" + mac("s3cret", body)}, "X-Github-Event": {"push"}}
	ev, err := g.ParseWebhook(sig, []byte(body), "s3cret")
	mustEvent(t, ev, err, Changed, "root", "r1", 1)
	if _, err := g.ParseWebhook(http.Header{"X-Github-Event": {"push"}}, []byte(body), "s3cret"); err != ErrSignature {
		t.Errorf("unsigned: %v", err)
	}
	sig.Set("X-Github-Event", "issues")
	if ev, _ := g.ParseWebhook(sig, []byte(body), "s3cret"); ev != nil {
		t.Errorf("ignored: %+v", ev)
	}
}
