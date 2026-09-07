package engine

import (
	"fmt"
	"testing"
	"time"

	"github.com/cristobaltormo/git-sync/internal/forge"
)

func TestCoalescing(t *testing.T) {
	h := newHarness(t, nil)
	e := h.e
	e.Enqueue(&Job{Key: "1", Owner: "alice", Name: "site", Why: "a"})
	e.Enqueue(&Job{Key: "1", Owner: "alice", Name: "site", Why: "b"})
	eq(t, e.queued(), 1)
	q := &e.queue
	q.mu.Lock()
	k := q.order[0]
	q.order = nil
	delete(q.pending, k)
	q.running[k] = true
	q.mu.Unlock()
	e.Enqueue(&Job{Key: "1", Why: "c"})
	e.Enqueue(&Job{Key: "1", Why: "d"})
	eq(t, len(q.dirty), 1)
	eq(t, e.queued(), 0)
}

func TestWorkersEndToEnd(t *testing.T) {
	h := newHarness(t, nil)
	for i := int64(1); i <= 6; i++ {
		h.src.set(mk(i, fmt.Sprintf("r%d", i)))
	}
	h.e.Start()
	h.e.Reconcile(false, "scan")
	isTrue(t, h.e.Drain(10*time.Second), "queue did not drain")
	h.e.Shutdown()
	eq(t, len(h.mir.calls), 6)
}

func TestWebhookBurstRunsOnce(t *testing.T) {
	h := newHarness(t, nil)
	h.src.set(mk(1, "site"))
	h.e.Start()
	start := time.Now()
	ev := &forge.Event{Kind: forge.Changed, Owner: "alice", Name: "site", ID: 1}
	h.e.HandleEvent(ev)
	h.e.HandleEvent(ev)
	isTrue(t, h.e.Drain(5*time.Second), "queue did not drain")
	h.e.Shutdown()
	isTrue(t, time.Since(start) >= hookDelay, "the job must wait for the debounce window")
	eq(t, len(h.mir.calls), 1)
}
