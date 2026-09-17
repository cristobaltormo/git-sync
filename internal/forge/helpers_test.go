package forge

import (
	"encoding/json"
	"io"
	"net/http"
	"net/http/httptest"
	"sync"
	"testing"

	"github.com/cristobaltormo/git-sync/internal/config"
	"github.com/cristobaltormo/git-sync/internal/logx"
)

type route func(w http.ResponseWriter, r *http.Request, body []byte)

type fakeServer struct {
	mu    sync.Mutex
	log   []string
	auth  string
	route route
}

func (f *fakeServer) ServeHTTP(w http.ResponseWriter, r *http.Request) {
	b, _ := io.ReadAll(r.Body)
	f.mu.Lock()
	f.log = append(f.log, r.Method+" "+r.URL.RequestURI())
	f.auth = r.Header.Get("Authorization")
	f.mu.Unlock()
	f.route(w, r, b)
}

func serve(t *testing.T, typ string, fn route) (*config.Config, *fakeServer) {
	logx.Silence()
	f := &fakeServer{route: fn}
	srv := httptest.NewServer(f)
	t.Cleanup(srv.Close)
	cfg := config.Default()
	cfg.Source.Type, cfg.Source.URL, cfg.Source.Token = typ, srv.URL, "src-token-123456"
	cfg.Listen.Secret = "s3cret-value-123"
	return cfg, f
}

func js(w http.ResponseWriter, v any) { json.NewEncoder(w).Encode(v) }

func mustEvent(t *testing.T, ev *Event, err error, kind Change, owner, name string, id int64) {
	t.Helper()
	if err != nil || ev == nil {
		t.Fatalf("event: %+v %v", ev, err)
	}
	if ev.Kind != kind || ev.Owner != owner || ev.Name != name || ev.ID != id {
		t.Fatalf("got %+v, want kind=%d %s/%s id=%d", ev, kind, owner, name, id)
	}
}
