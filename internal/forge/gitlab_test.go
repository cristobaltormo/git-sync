package forge

import (
	"encoding/base64"
	"encoding/json"
	"net/http"
	"strings"
	"testing"
	"time"
)

func gitlabRoute(hooks *[]map[string]any) route {
	project := func(id int, path, ns, vis string) map[string]any {
		return map[string]any{"id": id, "path": path, "path_with_namespace": ns + "/" + path,
			"description": "d", "visibility": vis, "archived": false, "default_branch": "main",
			"empty_repo": false, "mirror": nil, "topics": []string{"a"},
			"last_activity_at": "2026-09-24T19:29:15.121Z", "namespace": map[string]any{"full_path": ns}}
	}
	return func(w http.ResponseWriter, r *http.Request, body []byte) {
		p := r.URL.EscapedPath()
		switch {
		case p == "/api/v4/user":
			js(w, map[string]any{"username": "root", "is_admin": true})
		case p == "/api/v4/namespaces/root":
			js(w, map[string]any{"id": 1, "kind": "user"})
		case p == "/api/v4/namespaces/team%2Fweb":
			js(w, map[string]any{"id": 5, "kind": "group"})
		case p == "/api/v4/users/1/projects":
			gone := project(9, "old-deletion_scheduled-9", "root", "private")
			gone["marked_for_deletion_at"] = "2026-09-24"
			js(w, []any{project(1, "site", "root", "private"), project(2, "blog", "root", "internal"), gone})
		case p == "/api/v4/projects/root%2Fold-deletion_scheduled-9":
			gone := project(9, "old-deletion_scheduled-9", "root", "private")
			gone["marked_for_deletion_at"] = "2026-09-24"
			js(w, gone)
		case p == "/api/v4/groups/5/projects":
			page := r.URL.Query().Get("page")
			if page == "1" {
				w.Header().Set("X-Next-Page", "2")
				js(w, []any{project(3, "a", "team/web", "public")})
			} else {
				js(w, []any{project(4, "b", "team/web", "public")})
			}
		case p == "/api/v4/projects/root%2Fsite":
			js(w, project(1, "site", "root", "public"))
		case p == "/api/v4/projects/root%2Fgone":
			http.NotFound(w, r)
		case (p == "/api/v4/hooks" || p == "/api/v4/projects/root%2Fsite/hooks") && r.Method == "GET":
			js(w, *hooks)
		case (p == "/api/v4/hooks" || p == "/api/v4/projects/root%2Fsite/hooks") && r.Method == "POST":
			var h map[string]any
			json.Unmarshal(body, &h)
			h["id"] = len(*hooks) + 10
			*hooks = append(*hooks, h)
			w.WriteHeader(201)
			js(w, h)
		case strings.HasPrefix(p, "/api/v4/hooks/") && r.Method == "DELETE":
			*hooks = nil
			w.WriteHeader(204)
		case strings.HasPrefix(p, "/api/v4/projects/root%2Fsite/hooks/") && r.Method == "DELETE":
			*hooks = nil
			w.WriteHeader(204)
		default:
			http.NotFound(w, r)
		}
	}
}

func TestGitLab(t *testing.T) {
	var hooks []map[string]any
	cfg, srv := serve(t, "gitlab", gitlabRoute(&hooks))
	g := newGitLab(cfg)
	g.http.SetSleep(func(time.Duration) {})

	if id, err := g.Whoami(); err != nil || id.Login != "root" || !id.Admin {
		t.Fatalf("whoami: %+v %v", id, err)
	}
	if srv.auth != "Bearer src-token-123456" {
		t.Fatalf("auth: %s", srv.auth)
	}
	repos, err := g.List("root")
	if err != nil || len(repos) != 2 {
		t.Fatalf("list: %v %v", repos, err)
	}
	if repos[0].Private != true || repos[1].Private != true || repos[0].Name != "site" || repos[0].Owner != "root" {
		t.Fatalf("visibility internal must count as private: %+v %+v", repos[0], repos[1])
	}
	if repos[0].DefaultBranch != "main" || repos[0].Updated == "" || len(repos[0].Topics) != 1 {
		t.Fatalf("mapping: %+v", repos[0])
	}
	group, err := g.List("team/web")
	if err != nil || len(group) != 2 || group[0].Private {
		t.Fatalf("groups are listed page by page: %v %v", group, err)
	}
	if r, _ := g.Get("root", "site"); r == nil || r.Private {
		t.Fatalf("get: %+v", r)
	}
	if r, err := g.Get("root", "old-deletion_scheduled-9"); r != nil || err != nil {
		t.Fatalf("a project marked for deletion no longer exists: %v %v", r, err)
	}
	if r, err := g.Get("root", "gone"); r != nil || err != nil {
		t.Fatalf("404 is (nil, nil): %v %v", r, err)
	}
	if h, _ := g.GitHeader(); h != "Authorization: Basic "+base64.StdEncoding.EncodeToString([]byte("oauth2:src-token-123456")) {
		t.Fatalf("git header: %s", h)
	}
	if u := g.CloneURL(&Repo{Owner: "team/web", Name: "a"}); !strings.HasSuffix(u, "/team/web/a.git") {
		t.Fatalf("clone url: %s", u)
	}

	h := g.Hooks()
	if _, res, err := h.Global(0, false); err != nil || res != "created" {
		t.Fatalf("global: %v %v", res, err)
	}
	if _, res, _ := h.Global(0, false); res != "exists" {
		t.Fatalf("global again: %v", res)
	}
	if !h.RemoveGlobal(0) || len(hooks) != 0 {
		t.Fatal("remove global")
	}
	if res, err := h.Repo(&Repo{Owner: "root", Name: "site"}, false); err != nil || res != "created" {
		t.Fatalf("repo hook: %v %v", res, err)
	}
	if h.RemoveRepo(&Repo{Owner: "root", Name: "site"}) != 1 {
		t.Fatal("remove repo hook")
	}
}

func TestGitLabWebhooks(t *testing.T) {
	cfg, _ := serve(t, "gitlab", func(http.ResponseWriter, *http.Request, []byte) {})
	g := newGitLab(cfg)
	tok := http.Header{"X-Gitlab-Token": {"s3cret-value-123"}}
	push := `{"object_kind":"push","project_id":7,"project":{"id":7,"path_with_namespace":"team/web/site"}}`
	ev, err := g.ParseWebhook(tok, []byte(push), "s3cret-value-123")
	mustEvent(t, ev, err, Changed, "team/web", "site", 7)
	system := `{"event_name":"project_destroy","project_id":9,"path_with_namespace":"root/old"}`
	ev, err = g.ParseWebhook(tok, []byte(system), "s3cret-value-123")
	mustEvent(t, ev, err, Removed, "root", "old", 9)
	for name, h := range map[string]http.Header{"missing": {}, "wrong": {"X-Gitlab-Token": {"nope"}}} {
		if _, err := g.ParseWebhook(h, []byte(push), "s3cret-value-123"); err != ErrSignature {
			t.Errorf("%s token must be rejected: %v", name, err)
		}
	}
}

func TestRequestFailuresSurface(t *testing.T) {
	cfg, _ := serve(t, "gitlab", func(w http.ResponseWriter, r *http.Request, _ []byte) { w.WriteHeader(401) })
	g := newGitLab(cfg)
	_, err := g.Whoami()
	if err == nil || !strings.Contains(err.Error(), "rejected") {
		t.Fatalf("a rejected token must say so: %v", err)
	}
}
