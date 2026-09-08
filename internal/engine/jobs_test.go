package engine

import (
	"fmt"
	"strings"
	"testing"
	"time"

	"github.com/cristobaltormo/git-sync/internal/httpx"
)

func TestFailureBacksOffThenRecovers(t *testing.T) {
	h := newHarness(t, nil)
	h.src.set(mk(1, "site"))
	h.mir.fail = fmt.Errorf("network down")
	h.run(false)
	x, _ := h.e.state.get(1)
	isTrue(t, strings.Contains(x.LastError, "network down"), "error not recorded")
	isTrue(t, x.RetryAt > time.Now().Unix(), "no backoff")
	h.e.Reconcile(false, "scan")
	eq(t, h.e.queued(), 0)
	h.mir.fail = nil
	h.e.state.update(1, false, func(x *Entry) { x.RetryAt = time.Now().Unix() - 1 })
	h.run(false)
	x, _ = h.e.state.get(1)
	eq(t, x.LastError, "")
}

func TestBusyGitHubRetriesQuickly(t *testing.T) {
	h := newHarness(t, nil)
	h.src.set(mk(1, "site"))
	h.mir.fail = fmt.Errorf("git push failed: remote: Repository 'x/site' is disabled. | fatal: 403")
	h.run(false)
	x, _ := h.e.state.get(1)
	wait := x.RetryAt - time.Now().Unix()
	isTrue(t, wait > 0 && wait <= 12, fmt.Sprintf("a busy repo should be retried in about 10s, got %ds", wait))
	at, ok := h.e.nextRetry()
	isTrue(t, ok && at.Unix() == x.RetryAt, "the poller must know when to come back")
	isTrue(t, isBusy(&httpx.APIError{Status: 422, Msg: "Failed to update visibility. A previous visibility change is still in progress."}), "422 in progress")
	isTrue(t, !isBusy(&httpx.APIError{Status: 422, Msg: "Validation failed"}), "other 422s are real errors")
}
