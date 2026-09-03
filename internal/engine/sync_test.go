package engine

import (
	"strings"
	"testing"

	"github.com/cristobaltormo/git-sync/internal/config"
	"github.com/cristobaltormo/git-sync/internal/forge"
	"github.com/cristobaltormo/git-sync/internal/github"
)

func TestNewPrivateRepo(t *testing.T) {
	h := newHarness(t, nil)
	h.src.set(mk(1, "site", func(r *forge.Repo) { r.Description = "my site" }))
	h.run(false)
	eq(t, h.tgt.calls[0], "create alice-gh/site")
	eq(t, h.mir.calls, "[mirror alice/site alice-gh/site]")
	isTrue(t, h.tgt.repos["alice-gh/site"].Private, "should be private")
	x, ok := h.e.state.get(1)
	isTrue(t, ok && x.Managed && x.GHName == "site", "entry not managed")
	isTrue(t, x.LastError == "", "unexpected error")
}

func TestNoChangesNoCalls(t *testing.T) {
	h := newHarness(t, nil)
	h.src.set(mk(1, "site"))
	h.run(false)
	n := len(h.tgt.calls)
	h.e.Reconcile(false, "scan")
	eq(t, h.e.queued(), 0)
	eq(t, len(h.tgt.calls), n)
}

func TestExistingRepoNeedsAdoption(t *testing.T) {
	h := newHarness(t, nil)
	h.tgt.repos["alice-gh/site"] = &github.Repo{Name: "site", Private: true, DefaultBranch: "main"}
	h.src.set(mk(1, "site"))
	h.run(false)
	eq(t, len(h.mir.calls), 0)
	x, _ := h.e.state.get(1)
	isTrue(t, strings.Contains(x.LastError, "adopt_existing") && !x.Managed, "should be blocked")
	h.rebuild(func(c *config.Config) { c.Sync.AdoptExisting = true })
	h.e.state.update(1, false, func(x *Entry) { x.RetryAt = 0 })
	h.run(false)
	eq(t, len(h.mir.calls), 1)
	x, _ = h.e.state.get(1)
	isTrue(t, x.Managed, "should be managed now")
}

func TestUnmappedOwnerIgnored(t *testing.T) {
	h := newHarness(t, nil)
	h.src.set(mk(1, "x", func(r *forge.Repo) { r.Owner = "stranger" }))
	h.run(false)
	eq(t, len(h.tgt.calls), 0)
}
