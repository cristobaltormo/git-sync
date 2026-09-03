package engine

import (
	"fmt"
	"strings"
	"testing"
	"time"
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
