package engine

import (
	"testing"

	"github.com/cristobaltormo/git-sync/internal/config"
	"github.com/cristobaltormo/git-sync/internal/forge"
)

func TestWebhookRouting(t *testing.T) {
	h := newHarness(t, nil)
	h.e.HandleEvent(&forge.Event{Kind: forge.Changed, Owner: "stranger", Name: "x", ID: 7})
	eq(t, h.e.queued(), 0)
	h.e.HandleEvent(&forge.Event{Kind: forge.Changed, Owner: "alice", Name: "site", ID: 1})
	eq(t, h.e.queued(), 1)
	job := h.e.queue.pending["1"]
	isTrue(t, job.Repo == nil && job.Owner == "alice" && job.Name == "site", "webhook jobs fetch fresh data")
	h.e.HandleEvent(&forge.Event{Kind: forge.Removed, Owner: "alice", Name: "gone", ID: 2})
	select {
	case <-h.e.wake:
	default:
		t.Fatal("a removed repo should wake the poller")
	}
}

func TestWebhookForMissingRepoWakesPoller(t *testing.T) {
	h := newHarness(t, nil)
	h.e.Enqueue(&Job{Key: "5", Owner: "alice", Name: "ghost", Why: "webhook"})
	h.pump()
	select {
	case <-h.e.wake:
	default:
		t.Fatal("expected a wake-up")
	}
}

func TestHookSetup(t *testing.T) {
	h := newHarness(t, nil)
	h.src.hooks = &fakeHooks{}
	h.src.admin = true
	isTrue(t, h.e.SetupHooks(false), "setup")
	eq(t, h.e.HookMode(), "system")
	eq(t, h.e.SystemHook(), 42)

	h2 := newHarness(t, nil)
	h2.src.hooks = &fakeHooks{}
	h2.e.SetupHooks(false)
	eq(t, h2.e.HookMode(), "repo")
	h2.src.set(mk(1, "site"))
	h2.run(false)
	eq(t, h2.src.hooks.(*fakeHooks).repo, "[alice/site]")

	h3 := newHarness(t, nil)
	h3.e.SetupHooks(false)
	eq(t, h3.e.HookMode(), "none")

	h4 := newHarness(t, func(c *config.Config) { c.Hooks.Mode = "none" })
	h4.src.hooks = &fakeHooks{}
	h4.e.SetupHooks(false)
	eq(t, h4.e.HookMode(), "none")
}
