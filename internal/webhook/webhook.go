package webhook

import (
	"errors"
	"io"
	"net"
	"net/http"
	"time"

	"github.com/cristobaltormo/git-sync/internal/config"
	"github.com/cristobaltormo/git-sync/internal/forge"
	"github.com/cristobaltormo/git-sync/internal/logx"
)

const maxBody = 10 << 20

func Handler(src forge.Provider, secret func() string, deliver func(*forge.Event)) http.Handler {
	mux := http.NewServeMux()
	mux.HandleFunc("/health", func(w http.ResponseWriter, r *http.Request) {
		io.WriteString(w, "ok\n")
	})
	mux.HandleFunc("/", func(w http.ResponseWriter, r *http.Request) {
		if r.Method != http.MethodPost {
			http.NotFound(w, r)
			return
		}
		body, err := io.ReadAll(http.MaxBytesReader(w, r.Body, maxBody))
		if err != nil {
			http.Error(w, "body too large", http.StatusRequestEntityTooLarge)
			return
		}
		ev, err := src.ParseWebhook(r.Header, body, secret())
		switch {
		case errors.Is(err, forge.ErrSignature):
			logx.Warnf("webhook rejected: bad or missing signature from %s", r.RemoteAddr)
			http.Error(w, "bad signature", http.StatusForbidden)
			return
		case err != nil:
			http.Error(w, "bad request", http.StatusBadRequest)
			return
		}
		w.WriteHeader(http.StatusAccepted)
		io.WriteString(w, "ok\n")
		if ev != nil {
			logx.Debugf("webhook for %s/%s", ev.Owner, ev.Name)
			deliver(ev)
		}
	})
	return mux
}

func Listen(host string, port int, h http.Handler) (*http.Server, error) {
	addr := config.HostPort(host, port)
	ln, err := net.Listen("tcp", addr)
	if err != nil {
		return nil, config.Errorf("cannot listen on %s: %v", addr, err)
	}
	srv := &http.Server{
		Handler:           h,
		ReadHeaderTimeout: 10 * time.Second,
		ReadTimeout:       30 * time.Second,
		IdleTimeout:       30 * time.Second,
	}
	go srv.Serve(ln)
	return srv, nil
}
