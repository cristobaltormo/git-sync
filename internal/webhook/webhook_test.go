package webhook

import (
	"crypto/hmac"
	"crypto/sha256"
	"encoding/hex"
	"net/http"
	"net/http/httptest"
	"strings"
	"testing"

	"github.com/cristobaltormo/git-sync/internal/config"
	"github.com/cristobaltormo/git-sync/internal/forge"
	"github.com/cristobaltormo/git-sync/internal/logx"
)

func sign(secret, body string) string {
	m := hmac.New(sha256.New, []byte(secret))
	m.Write([]byte(body))
	return hex.EncodeToString(m.Sum(nil))
}

func TestListener(t *testing.T) {
	logx.Silence()
	cfg := config.Default()
	cfg.Source.Type, cfg.Source.URL, cfg.Source.Token = "forgejo", "https://git.example.com", "src-token-123456"
	src, err := forge.New(cfg)
	if err != nil {
		t.Fatal(err)
	}
	var got []*forge.Event
	srv := httptest.NewServer(Handler(src, func() string { return "s3cret" }, func(e *forge.Event) { got = append(got, e) }))
	defer srv.Close()

	post := func(event, body, sig string) int {
		req, _ := http.NewRequest("POST", srv.URL+"/hook", strings.NewReader(body))
		req.Header.Set("X-Gitea-Event", event)
		if sig != "" {
			req.Header.Set("X-Gitea-Signature", sig)
		}
		resp, err := http.DefaultClient.Do(req)
		if err != nil {
			t.Fatal(err)
		}
		resp.Body.Close()
		return resp.StatusCode
	}
	body := `{"repository":{"id":1,"full_name":"alice/site"}}`
	for name, code := range map[string]int{
		"unsigned": post("push", body, ""),
		"forged":   post("push", body, "deadbeef"),
		"bad json": post("push", "not json", sign("s3cret", "not json")),
		"ok":       post("push", body, sign("s3cret", body)),
		"ignored":  post("issues", body, sign("s3cret", body)),
	} {
		want := map[string]int{"unsigned": 403, "forged": 403, "bad json": 400, "ok": 202, "ignored": 202}[name]
		if code != want {
			t.Errorf("%s: got %d, want %d", name, code, want)
		}
	}
	if len(got) != 1 || got[0].Owner != "alice" || got[0].Name != "site" || got[0].ID != 1 {
		t.Fatalf("delivered: %+v", got)
	}
	if r, err := http.Get(srv.URL + "/health"); err != nil || r.StatusCode != 200 {
		t.Fatalf("health: %v", err)
	}
	if r, _ := http.Get(srv.URL + "/hook"); r.StatusCode != 404 {
		t.Fatalf("GET /hook should be 404, got %d", r.StatusCode)
	}
}
