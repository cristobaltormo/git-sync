package httpx

import (
	"io"
	"net/http"
	"net/http/httptest"
	"testing"
	"time"

	"github.com/cristobaltormo/git-sync/internal/logx"
)

func TestRetriesServerErrorsAndRateLimits(t *testing.T) {
	logx.Silence()
	n := 0
	srv := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		n++
		switch n {
		case 1:
			w.WriteHeader(502)
		case 2:
			w.Header().Set("Retry-After", "3")
			w.WriteHeader(403)
		default:
			io.WriteString(w, `{"ok":true}`)
		}
	}))
	defer srv.Close()
	c := New(nil)
	var slept []time.Duration
	c.SetSleep(func(d time.Duration) { slept = append(slept, d) })
	r, err := c.Do("GET", srv.URL, nil)
	if err != nil || !r.OK() {
		t.Fatalf("should have recovered: %v %v", r, err)
	}
	if n != 3 || len(slept) != 2 || slept[1] != 4*time.Second {
		t.Fatalf("calls=%d sleeps=%v", n, slept)
	}
}

func TestGivesUpOnNetworkErrors(t *testing.T) {
	c := New(nil)
	c.SetSleep(func(time.Duration) {})
	if _, err := c.Do("GET", "http://127.0.0.1:1/x", nil); err == nil {
		t.Fatal("expected an error")
	}
}

func TestClientErrorsAreReturnedNotRetried(t *testing.T) {
	n := 0
	srv := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		n++
		w.WriteHeader(404)
	}))
	defer srv.Close()
	r, err := New(nil).Do("GET", srv.URL, nil)
	if err != nil || r.Status != 404 || n != 1 {
		t.Fatalf("status=%v calls=%d err=%v", r, n, err)
	}
}
