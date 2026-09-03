package engine

import (
	"testing"
)

func TestStateSurvivesRestart(t *testing.T) {
	h := synced(t, nil)
	h.rebuild(nil)
	x, ok := h.e.state.get(1)
	isTrue(t, ok && x.Managed && x.GH != nil, "state not reloaded")
	h.e.Reconcile(false, "scan")
	eq(t, h.e.queued(), 0)
}
