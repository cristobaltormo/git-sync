package github

import (
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

type call struct{ method, path, body string }

func client(t *testing.T, h http.HandlerFunc) (*Client, *[]call) {
	logx.Silence()
	var mu sync.Mutex
	var calls []call
	srv := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		b, _ := io.ReadAll(r.Body)
		mu.Lock()
		calls = append(calls, call{r.Method, r.URL.Path, string(b)})
		mu.Unlock()
		if r.Header.Get("Authorization") != "Bearer gh-token-123456" {
			w.WriteHeader(401)
			return
		}
		h(w, r)
	}))
	t.Cleanup(srv.Close)
	cfg := config.Default()
	cfg.GitHub.Token, cfg.GitHub.APIURL = "gh-token-123456", srv.URL
	c := New(cfg, false)
	c.HTTP().SetSleep(func(time.Duration) {})
	return c, &calls
}

func TestCreateUnderUserAndOrganization(t *testing.T) {
	c, calls := client(t, func(w http.ResponseWriter, r *http.Request) {
		switch r.URL.Path {
		case "/user":
			w.Header().Set("X-OAuth-Scopes", "repo, delete_repo")
			io.WriteString(w, `{"login":"alice-gh"}`)
		case "/users/my-org":
			io.WriteString(w, `{"type":"Organization"}`)
		case "/users/bob":
			io.WriteString(w, `{"type":"User"}`)
		default:
			w.WriteHeader(201)
			io.WriteString(w, `{"name":"site","private":true,"owner":{"login":"x"}}`)
		}
	})
	if _, err := c.Create("alice-gh", "site", "desc", ""); err != nil {
		t.Fatal(err)
	}
	if _, err := c.Create("my-org", "site", "", "https://x.example"); err != nil {
		t.Fatal(err)
	}
	if _, err := c.Create("bob", "site", "", ""); err == nil || !strings.Contains(err.Error(), "cannot create repos under the user") {
		t.Fatalf("want a clear error for another user, got %v", err)
	}
	var paths []string
	for _, x := range *calls {
		if x.method == "POST" {
			paths = append(paths, x.path)
			if !strings.Contains(x.body, `"private":true`) {
				t.Errorf("repos are always created private: %s", x.body)
			}
		}
	}
	if strings.Join(paths, " ") != "/user/repos /orgs/my-org/repos" {
		t.Fatalf("paths: %v", paths)
	}
	login, scopes, _ := c.Whoami()
	if login != "alice-gh" || scopes == nil || !strings.Contains(*scopes, "delete_repo") {
		t.Fatalf("whoami: %s %v", login, scopes)
	}
}

func TestGet(t *testing.T) {
	c, _ := client(t, func(w http.ResponseWriter, r *http.Request) {
		switch r.URL.Path {
		case "/repos/o/missing":
			w.WriteHeader(404)
		case "/repos/o/forbidden":
			w.WriteHeader(403)
			io.WriteString(w, `{"message":"nope"}`)
		default:
			io.WriteString(w, `{"name":"site","description":null,"topics":["b","a"],"owner":{"login":"o"}}`)
		}
	})
	if r, err := c.Get("o", "missing"); r != nil || err != nil {
		t.Fatalf("404 must be (nil, nil): %v %v", r, err)
	}
	if _, err := c.Get("o", "forbidden"); err == nil || !strings.Contains(err.Error(), "403") {
		t.Fatalf("403 must be an error: %v", err)
	}
	r, err := c.Get("o", "site")
	if err != nil {
		t.Fatal(err)
	}
	if s := Snapshot(r); s.Description != "" || strings.Join(s.Topics, ",") != "a,b" {
		t.Fatalf("snapshot: %+v", s)
	}
}

func TestDryRunNeverMutates(t *testing.T) {
	c, calls := client(t, func(w http.ResponseWriter, r *http.Request) { io.WriteString(w, `{}`) })
	c.dry = true
	c.Patch("o", "r", map[string]any{"private": false})
	c.SetTopics("o", "r", []string{"a"})
	c.Delete("o", "r")
	if len(*calls) != 0 {
		t.Fatalf("dry run made calls: %v", *calls)
	}
}

func TestCleanTopics(t *testing.T) {
	got := CleanTopics([]string{"Foo Bar", "foo-bar", "a_b", "--x--", "ok", "gitsync"}, "gitsync")
	if strings.Join(got, ",") != "foo-bar,a-b,x,ok" {
		t.Fatalf("got %v", got)
	}
	var many []string
	for i := 0; i < 40; i++ {
		many = append(many, "t"+strings.Repeat("x", i))
	}
	if len(CleanTopics(many, "")) != 20 {
		t.Fatal("at most 20 topics")
	}
}
