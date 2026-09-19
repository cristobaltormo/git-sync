package forge

import (
	"encoding/json"
	"io"
	"net/http"
	"strings"
	"testing"
	"time"
)

func TestOneDev(t *testing.T) {
	setting := map[string]json.RawMessage{"branchProtections": json.RawMessage("[]"), "webHooks": json.RawMessage("[]")}
	cfg, srv := serve(t, "onedev", func(w http.ResponseWriter, r *http.Request, body []byte) {
		p := r.URL.Path
		project := func(id int, name, path string, forked any) map[string]any {
			return map[string]any{"id": id, "name": name, "path": path, "description": "hola", "forkedFromId": forked, "parentId": nil}
		}
		switch {
		case p == "/~api/users/me":
			js(w, map[string]any{"id": 1, "name": "admin"})
		case p == "/~api/projects":
			js(w, []any{project(1, "p1", "p1", nil), project(2, "child", "parent/child", 1), project(3, "other", "parent/other", nil)})
		case p == "/~api/projects/ids/p1":
			io.WriteString(w, "1")
		case p == "/~api/projects/ids/parent/child":
			io.WriteString(w, "2")
		case strings.HasPrefix(p, "/~api/projects/ids/"):
			http.Error(w, "nope", 500)
		case p == "/~api/projects/2":
			js(w, project(2, "child", "parent/child", 1))
		case p == "/~api/projects/1/setting" && r.Method == "GET":
			js(w, setting)
		case p == "/~api/projects/1/setting" && r.Method == "POST":
			json.Unmarshal(body, &setting)
			js(w, 1)
		default:
			http.NotFound(w, r)
		}
	})
	o := newOneDev(cfg)
	o.http.SetSleep(func(time.Duration) {})
	root, err := o.List(OneDevRoot)
	if err != nil || len(root) != 1 || root[0].Name != "p1" || root[0].Owner != OneDevRoot || root[0].Full() != "p1" {
		t.Fatalf("root projects: %+v %v", root, err)
	}
	child, _ := o.List("parent")
	if len(child) != 2 || !child[0].Fork || child[0].Owner != "parent" || !child[0].Private {
		t.Fatalf("nested projects: %+v", child)
	}
	if srv.auth != "Bearer src-token-123456" {
		t.Fatalf("auth: %s", srv.auth)
	}
	if got, _ := o.Get("parent", "child"); got == nil || got.ID != 2 {
		t.Fatalf("get: %+v", got)
	}
	if got, err := o.Get("parent", "missing"); got != nil || err != nil {
		t.Fatalf("a missing project is (nil, nil): %v %v", got, err)
	}
	if ok, _ := o.OwnerExists(OneDevRoot); !ok {
		t.Fatal("the root always exists")
	}
	if u := o.CloneURL(&Repo{Owner: "parent", Name: "child"}); !strings.HasSuffix(u, "/parent/child.git") {
		t.Fatalf("clone url: %s", u)
	}
	if u := o.CloneURL(&Repo{Owner: OneDevRoot, Name: "p1"}); !strings.HasSuffix(u, "/p1.git") || strings.Contains(u, "//p1") {
		t.Fatalf("root clone url: %s", u)
	}
	h := o.Hooks()
	repo := &Repo{Owner: OneDevRoot, Name: "p1"}
	if res, err := h.Repo(repo, false); err != nil || res != "created" {
		t.Fatalf("hook: %v %v", res, err)
	}
	var stored []odWebHook
	json.Unmarshal(setting["webHooks"], &stored)
	if len(stored) != 1 || stored[0].PostURL != cfg.HookURL() || stored[0].Secret != cfg.Listen.Secret || stored[0].EventTypes[0] != "CODE_PUSH" {
		t.Fatalf("stored webhook: %+v", stored)
	}
	if _, ok := setting["branchProtections"]; !ok {
		t.Fatal("the rest of the project settings must be sent back untouched")
	}
	if res, _ := h.Repo(repo, false); res != "exists" {
		t.Fatalf("hook again: %s", res)
	}
	if h.RemoveRepo(repo) != 1 {
		t.Fatal("remove")
	}
}

func TestOneDevWebhooks(t *testing.T) {
	cfg, _ := serve(t, "onedev", func(w http.ResponseWriter, r *http.Request, _ []byte) {
		js(w, map[string]any{"id": 2, "name": "child", "path": "parent/child"})
	})
	o := newOneDev(cfg)
	body := `{"projectId":2,"refName":"refs/heads/main","type":"RefUpdated"}`
	signed := func(v string) http.Header {
		h := http.Header{}
		h.Set("X-OneDev-Signature", v)
		return h
	}
	ev, err := o.ParseWebhook(signed("s3cret"), []byte(body), "s3cret")
	mustEvent(t, ev, err, Changed, "parent", "child", 2)
	if _, err := o.ParseWebhook(signed("nope"), []byte(body), "s3cret"); err != ErrSignature {
		t.Errorf("wrong secret: %v", err)
	}
}
