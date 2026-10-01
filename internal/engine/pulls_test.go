package engine

import (
	"io"
	"net/http"
	"net/http/httptest"
	"strings"
	"sync"
	"testing"
	"time"

	"github.com/cristobaltormo/git-sync/internal/config"
	"github.com/cristobaltormo/git-sync/internal/github"
)

func TestPullRequestsAreNeverTouchedByDefault(t *testing.T) {
	h := synced(t, nil)
	h.tgt.pulls["alice-gh"] = []github.Pull{{Owner: "alice-gh", Repo: "site", Number: 1}}
	h.e.checkPulls()
	eq(t, len(h.tgt.comments), 0)
}

func TestPullRequestCommentOnlyForNewOnes(t *testing.T) {
	h := synced(t, func(c *config.Config) { c.Pulls.Mode = "comment" })
	h.tgt.pulls["alice-gh"] = []github.Pull{{Owner: "alice-gh", Repo: "site", Number: 1}}
	h.e.checkPulls()
	eq(t, len(h.tgt.comments), 0)

	h.tgt.pulls["alice-gh"] = append(h.tgt.pulls["alice-gh"], github.Pull{Owner: "alice-gh", Repo: "site", Number: 2},
		github.Pull{Owner: "alice-gh", Repo: "not-ours", Number: 3})
	h.e.checkPulls()
	h.e.checkPulls()
	eq(t, len(h.tgt.comments), 1)
	isTrue(t, strings.HasPrefix(h.tgt.comments[0], "alice-gh/site#2: This repository is a read-only mirror"), h.tgt.comments[0])
}

func TestNotificationsReachTheWebhook(t *testing.T) {
	var mu sync.Mutex
	var bodies []string
	srv := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		b, _ := io.ReadAll(r.Body)
		mu.Lock()
		bodies = append(bodies, string(b))
		mu.Unlock()
	}))
	defer srv.Close()
	h := newHarness(t, func(c *config.Config) {
		c.Filter.NewRepos = "review"
		c.Notify.URL, c.Notify.Format = srv.URL, "slack"
	})
	h.src.set(mk(1, "site"))
	h.run(false)
	h.run(false)
	deadline := time.Now().Add(2 * time.Second)
	for time.Now().Before(deadline) {
		mu.Lock()
		n := len(bodies)
		mu.Unlock()
		if n > 0 {
			break
		}
		time.Sleep(10 * time.Millisecond)
	}
	time.Sleep(50 * time.Millisecond)
	mu.Lock()
	defer mu.Unlock()
	eq(t, len(bodies), 1)
	isTrue(t, strings.Contains(bodies[0], "alice/site") && strings.Contains(bodies[0], `"text"`), bodies[0])
}
