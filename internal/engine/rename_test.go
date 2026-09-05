package engine

import (
	"strings"
	"testing"

	"github.com/cristobaltormo/git-sync/internal/github"
)

func TestRename(t *testing.T) {
	h := newHarness(t, nil)
	h.src.set(mk(1, "old"))
	h.run(false)
	h.src.set(mk(1, "new", upd("2026-02-01T00:00:00Z")))
	h.run(false)
	isTrue(t, h.tgt.has("alice-gh/new") && !h.tgt.has("alice-gh/old"), "rename not applied")
	x, _ := h.e.state.get(1)
	eq(t, x.GHName, "new")
	for _, c := range h.tgt.calls[1:] {
		isTrue(t, !strings.HasPrefix(c, "create"), "renamed repo was created again")
	}
}

func TestRenameOntoExistingIsBlocked(t *testing.T) {
	h := newHarness(t, nil)
	h.src.set(mk(1, "old"))
	h.run(false)
	h.tgt.repos["alice-gh/new"] = &github.Repo{Name: "new"}
	h.src.set(mk(1, "new", upd("2026-02-01T00:00:00Z")))
	h.run(false)
	isTrue(t, h.tgt.has("alice-gh/old") && h.tgt.has("alice-gh/new"), "nothing may be touched")
	x, _ := h.e.state.get(1)
	isTrue(t, strings.Contains(x.LastError, "already exists"), "expected a clear error")
}
