package forge

import (
	"encoding/base64"
	"encoding/json"
	"io"
	"net/http"
	"strings"
	"testing"
	"time"
)

func TestBitbucketCloud(t *testing.T) {
	var srvURL string
	var hooks []map[string]any
	cfg, srv := serve(t, "bitbucket-cloud", func(w http.ResponseWriter, r *http.Request, body []byte) {
		p := r.URL.Path
		repo := func(uuid, slug string, private bool, main any) map[string]any {
			return map[string]any{"uuid": uuid, "slug": slug, "full_name": "ws/" + slug, "is_private": private,
				"description": "d", "website": "https://x.example", "updated_on": "2026-09-01T10:00:00Z",
				"mainbranch": main, "parent": nil}
		}
		switch {
		case p == "/2.0/user":
			http.Error(w, "no", 403)
		case p == "/2.0/repositories" && r.URL.Query().Get("role") == "member":
			js(w, map[string]any{"values": []any{}})
		case p == "/2.0/repositories/ws" && r.URL.Query().Get("page") == "":
			js(w, map[string]any{"values": []any{repo("{u1}", "a", true, map[string]any{"name": "main"})}, "next": srvURL + "/2.0/repositories/ws?page=2"})
		case p == "/2.0/repositories/ws":
			js(w, map[string]any{"values": []any{repo("{u2}", "b", false, nil)}})
		case p == "/2.0/repositories/ws/a":
			js(w, repo("{u1}", "a", true, map[string]any{"name": "main"}))
		case p == "/2.0/repositories/ws/gone":
			http.NotFound(w, r)
		case p == "/2.0/workspaces/ws":
			js(w, map[string]any{"slug": "ws"})
		case p == "/2.0/repositories/ws/a/hooks" && r.Method == "GET":
			js(w, map[string]any{"values": hooks})
		case p == "/2.0/repositories/ws/a/hooks" && r.Method == "POST":
			var h map[string]any
			json.Unmarshal(body, &h)
			h["uuid"] = "{h1}"
			hooks = append(hooks, h)
			w.WriteHeader(201)
			js(w, h)
		case strings.HasPrefix(p, "/2.0/repositories/ws/a/hooks/") && r.Method == "DELETE":
			hooks = nil
			w.WriteHeader(204)
		default:
			http.NotFound(w, r)
		}
	})
	srvURL = cfg.Source.URL
	b := newBitbucketCloud(cfg)
	b.http.SetSleep(func(time.Duration) {})
	if id, err := b.Whoami(); err != nil || id.Login != "token" {
		t.Fatalf("repository tokens cannot read /user: %+v %v", id, err)
	}
	repos, err := b.List("ws")
	if err != nil || len(repos) != 2 {
		t.Fatalf("pagination follows next: %v %v", repos, err)
	}
	if !repos[0].Private || repos[0].DefaultBranch != "main" || repos[0].Empty || repos[0].Website != "https://x.example" {
		t.Fatalf("mapping: %+v", repos[0])
	}
	if repos[1].Private || !repos[1].Empty || repos[0].ID == repos[1].ID {
		t.Fatalf("no main branch means empty, ids come from the uuid: %+v %+v", repos[0], repos[1])
	}
	if srv.auth != "Bearer src-token-123456" {
		t.Fatalf("auth: %s", srv.auth)
	}
	if got, _ := b.Get("ws", "a"); got == nil || got.Name != "a" {
		t.Fatalf("get: %+v", got)
	}
	if got, _ := b.Get("ws", "gone"); got != nil {
		t.Fatal("404")
	}
	if ok, _ := b.OwnerExists("ws"); !ok {
		t.Fatal("workspace")
	}
	if h, _ := b.GitHeader(); h != "Authorization: Basic "+base64.StdEncoding.EncodeToString([]byte("x-token-auth:src-token-123456")) {
		t.Fatalf("git header: %s", h)
	}
	h := b.Hooks()
	repo := &Repo{Owner: "ws", Name: "a"}
	if res, err := h.Repo(repo, false); err != nil || res != "created" {
		t.Fatalf("hook: %v %v", res, err)
	}
	hooks[0]["url"] = cfg.HookURL()
	if res, _ := h.Repo(repo, false); res != "exists" {
		t.Fatalf("hook again: %s", res)
	}
	if h.RemoveRepo(repo) != 1 {
		t.Fatal("remove")
	}

	body := `{"repository":{"uuid":"{u1}","full_name":"ws/a"}}`
	hdr := http.Header{"X-Hub-Signature": {"sha256=" + mac("s3cret", body)}, "X-Event-Key": {"repo:push"}}
	ev, err := b.ParseWebhook(hdr, []byte(body), "s3cret")
	mustEvent(t, ev, err, Changed, "ws", "a", hashID("{u1}"))
	if _, err := b.ParseWebhook(http.Header{"X-Event-Key": {"repo:push"}}, []byte(body), "s3cret"); err != ErrSignature {
		t.Errorf("unsigned: %v", err)
	}
}

func TestBitbucketServer(t *testing.T) {
	var hooks []map[string]any
	cfg, srv := serve(t, "bitbucket-server", func(w http.ResponseWriter, r *http.Request, body []byte) {
		p := r.URL.Path
		repo := func(id int, slug string, public bool) map[string]any {
			return map[string]any{"id": id, "slug": slug, "public": public, "archived": id == 2, "description": "d",
				"project": map[string]any{"key": "PROJ"}}
		}
		switch {
		case p == "/plugins/servlet/applinks/whoami":
			io.WriteString(w, "alice\n")
		case p == "/rest/api/1.0/projects/PROJ":
			js(w, map[string]any{"key": "PROJ"})
		case p == "/rest/api/1.0/projects/PROJ/repos" && r.URL.Query().Get("start") == "0":
			js(w, map[string]any{"values": []any{repo(1, "a", false)}, "isLastPage": false, "nextPageStart": 1})
		case p == "/rest/api/1.0/projects/PROJ/repos":
			js(w, map[string]any{"values": []any{repo(2, "b", true)}, "isLastPage": true})
		case p == "/rest/api/1.0/projects/PROJ/repos/a":
			js(w, repo(1, "a", false))
		case p == "/rest/api/1.0/projects/PROJ/repos/gone":
			http.NotFound(w, r)
		case p == "/rest/api/1.0/projects/PROJ/repos/a/webhooks" && r.Method == "GET":
			js(w, map[string]any{"values": hooks})
		case p == "/rest/api/1.0/projects/PROJ/repos/a/webhooks" && r.Method == "POST":
			var h map[string]any
			json.Unmarshal(body, &h)
			h["id"] = 5
			hooks = append(hooks, h)
			w.WriteHeader(201)
			js(w, h)
		case p == "/rest/api/1.0/projects/PROJ/repos/a/webhooks/5" && r.Method == "DELETE":
			hooks = nil
			w.WriteHeader(204)
		default:
			http.NotFound(w, r)
		}
	})
	b := newBitbucketServer(cfg)
	b.http.SetSleep(func(time.Duration) {})
	if id, err := b.Whoami(); err != nil || id.Login != "alice" {
		t.Fatalf("whoami: %+v %v", id, err)
	}
	repos, err := b.List("PROJ")
	if err != nil || len(repos) != 2 || !repos[0].Private || repos[1].Private || !repos[1].Archived {
		t.Fatalf("paging and mapping: %+v %v", repos, err)
	}
	if srv.auth != "Bearer src-token-123456" {
		t.Fatalf("auth: %s", srv.auth)
	}
	if got, _ := b.Get("PROJ", "gone"); got != nil {
		t.Fatal("404")
	}
	if u := b.CloneURL(repos[0]); !strings.HasSuffix(u, "/scm/proj/a.git") {
		t.Fatalf("clone url must use the lowercase project key: %s", u)
	}
	if h, _ := b.GitHeader(); h != "Authorization: Bearer src-token-123456" {
		t.Fatalf("git header: %s", h)
	}
	h := b.Hooks()
	repo := &Repo{Owner: "PROJ", Name: "a"}
	if res, err := h.Repo(repo, false); err != nil || res != "created" {
		t.Fatalf("hook: %v %v", res, err)
	}
	hooks[0]["url"] = cfg.HookURL()
	hooks[0]["id"] = 5
	if h.RemoveRepo(repo) != 1 {
		t.Fatal("remove")
	}
	body := `{"repository":{"id":1,"slug":"a","project":{"key":"PROJ"}}}`
	hdr := http.Header{"X-Hub-Signature": {"sha256=" + mac("s3cret", body)}, "X-Event-Key": {"repo:refs_changed"}}
	ev, err := b.ParseWebhook(hdr, []byte(body), "s3cret")
	mustEvent(t, ev, err, Changed, "PROJ", "a", 1)
}
