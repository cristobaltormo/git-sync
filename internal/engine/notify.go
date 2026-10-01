package engine

import (
	"bytes"
	"encoding/json"
	"fmt"
	"net/http"
	"slices"
	"time"

	"github.com/cristobaltormo/git-sync/internal/logx"
)

const (
	EventPending     = "pending"
	EventFailing     = "failing"
	EventPullRequest = "pull_request"
)

type Event struct {
	Kind string
	Repo string
	Info string
}

func (ev Event) message() string {
	switch ev.Kind {
	case EventPending:
		return fmt.Sprintf("New repository %s is waiting for your decision. Run `gitsync repos` to sync or ignore it.", ev.Repo)
	case EventFailing:
		return fmt.Sprintf("%s keeps failing to sync: %s", ev.Repo, ev.Info)
	case EventPullRequest:
		return fmt.Sprintf("A pull request was opened on the GitHub mirror of %s: %s", ev.Repo, ev.Info)
	}
	return ev.Repo
}

func (e *Engine) notify(ev Event) {
	n := e.Config().Notify
	if n.URL == "" || e.dry || !slices.Contains(n.Events, ev.Kind) {
		return
	}
	go func() {
		if err := post(n.URL, n.Format, ev); err != nil {
			logx.Warnf("notification failed: %v", err)
		}
	}()
}

func post(url, format string, ev Event) error {
	msg := ev.message()
	var body []byte
	ctype := "application/json"
	switch format {
	case "text":
		body, ctype = []byte(msg), "text/plain; charset=utf-8"
	case "slack":
		body, _ = json.Marshal(map[string]string{"text": msg})
	case "discord":
		body, _ = json.Marshal(map[string]string{"content": msg})
	default:
		body, _ = json.Marshal(map[string]string{
			"event": ev.Kind, "repo": ev.Repo, "message": msg, "time": time.Now().UTC().Format(time.RFC3339),
		})
	}
	req, err := http.NewRequest("POST", url, bytes.NewReader(body))
	if err != nil {
		return err
	}
	req.Header.Set("Content-Type", ctype)
	req.Header.Set("Title", "gitsync")
	req.Header.Set("User-Agent", "gitsync")
	c := &http.Client{Timeout: 10 * time.Second}
	resp, err := c.Do(req)
	if err != nil {
		return err
	}
	resp.Body.Close()
	if resp.StatusCode >= 300 {
		return fmt.Errorf("%s answered %s", url, resp.Status)
	}
	return nil
}

func SendTest(url, format string) error {
	return post(url, format, Event{Kind: EventPending, Repo: "my-org/example"})
}
