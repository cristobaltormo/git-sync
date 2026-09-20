package engine

import (
	"fmt"
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

func TestPublicOpensOnlyAfterPush(t *testing.T) {
	h := newHarness(t, nil)
	h.src.set(mk(1, "site", func(r *forge.Repo) { r.Private = false; r.Description = "d"; r.Topics = []string{"Python"} }))
	h.run(false)
	eq(t, events[0], "create")
	eq(t, events[1], "mirror")
	eq(t, events[2], "patch private=false")
	g := h.tgt.repos["alice-gh/site"]
	isTrue(t, !g.Private, "should be public")
	eq(t, g.Topics, "[python]")
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

func TestVisibilityBothWays(t *testing.T) {
	h := newHarness(t, nil)
	h.src.set(mk(1, "site"))
	h.run(false)
	h.src.set(mk(1, "site", func(r *forge.Repo) { r.Private = false }, upd("2026-02-01T00:00:00Z")))
	h.run(false)
	isTrue(t, !h.tgt.repos["alice-gh/site"].Private, "should have opened")
	events = nil
	h.src.set(mk(1, "site", upd("2026-03-01T00:00:00Z")))
	h.run(false)
	isTrue(t, h.tgt.repos["alice-gh/site"].Private, "should have closed")
	eq(t, events[0], "patch private=true")
}

func TestVisibilityOffLeavesGitHubAlone(t *testing.T) {
	h := newHarness(t, func(c *config.Config) { c.Sync.Visibility = false })
	h.src.set(mk(1, "site", func(r *forge.Repo) { r.Private = false }))
	h.run(false)
	isTrue(t, h.tgt.repos["alice-gh/site"].Private, "must stay private")
}

func TestMetadataAndDefaultBranch(t *testing.T) {
	h := newHarness(t, nil)
	h.src.set(mk(1, "site"))
	h.run(false)
	h.src.set(mk(1, "site", func(r *forge.Repo) {
		r.Description, r.Website, r.DefaultBranch, r.Topics = "new text", "https://x.example", "trunk", []string{"a", "b"}
	}, upd("2026-02-01T00:00:00Z")))
	h.run(false)
	g := h.tgt.repos["alice-gh/site"]
	eq(t, *g.Description+"|"+*g.Homepage+"|"+g.DefaultBranch+"|"+fmt.Sprint(g.Topics),
		"new text|https://x.example|trunk|[a b]")
}

func TestMetadataOff(t *testing.T) {
	h := newHarness(t, func(c *config.Config) { c.Sync.Metadata = false })
	h.src.set(mk(1, "site", func(r *forge.Repo) { r.Description = "x"; r.Topics = []string{"a"} }))
	h.run(false)
	g := h.tgt.repos["alice-gh/site"]
	eq(t, *g.Description+fmt.Sprint(len(g.Topics)), "0")
}

func TestArchiveFlow(t *testing.T) {
	h := newHarness(t, nil)
	h.src.set(mk(1, "site"))
	h.run(false)
	h.src.set(mk(1, "site", func(r *forge.Repo) { r.Archived = true }, upd("2026-02-01T00:00:00Z")))
	h.run(false)
	isTrue(t, h.tgt.repos["alice-gh/site"].Archived, "should be archived")
	h.tgt.calls, h.mir.calls = nil, nil
	h.run(true)
	eq(t, len(h.mir.calls), 0)
	eq(t, len(h.tgt.calls), 0)
	h.src.set(mk(1, "site", upd("2026-03-01T00:00:00Z")))
	h.run(false)
	isTrue(t, !h.tgt.repos["alice-gh/site"].Archived, "should be unarchived")
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

func TestRefusedDefaultBranchIsTriedOnce(t *testing.T) {
	h := newHarness(t, nil)
	h.tgt.refuseBranch = true
	h.src.set(mk(1, "site", func(r *forge.Repo) { r.DefaultBranch = "master" }))
	h.run(false)
	h.src.set(mk(1, "site", func(r *forge.Repo) { r.DefaultBranch = "master" }, upd("2026-02-01T00:00:00Z")))
	h.run(false)
	tries := 0
	for _, c := range h.tgt.calls {
		if strings.Contains(c, "default_branch") {
			tries++
		}
	}
	eq(t, tries, 1)
	x, _ := h.e.state.get(1)
	eq(t, x.LastError, "")
}
