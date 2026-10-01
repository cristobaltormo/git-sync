package engine

import (
	"strings"
	"testing"

	"github.com/cristobaltormo/git-sync/internal/config"
	"github.com/cristobaltormo/git-sync/internal/forge"
)

func yes() *bool { b := true; return &b }
func no() *bool  { b := false; return &b }

func withRules(rules map[string]*config.RepoRule) func(*config.Config) {
	return func(c *config.Config) { c.Repos = &config.Repos{Rules: rules} }
}

func TestNewReposReviewKeepsThemOutUntilDecided(t *testing.T) {
	h := newHarness(t, func(c *config.Config) { c.Filter.NewRepos = "review" })
	h.src.set(mk(1, "site"))
	h.run(false)
	isTrue(t, !h.tgt.has("alice-gh/site"), "undecided repo must not reach GitHub")
	x, has := h.e.state.get(1)
	isTrue(t, has && x.Pending != 0 && !x.Managed, "the repo must be recorded as pending")
	eq(t, len(h.tgt.calls), 0)

	h.rebuild(func(c *config.Config) {
		c.Filter.NewRepos = "review"
		withRules(map[string]*config.RepoRule{"alice/site": {Sync: yes()}})(c)
	})
	h.run(false)
	isTrue(t, h.tgt.has("alice-gh/site"), "allowed repo must be synced")
	x, _ = h.e.state.get(1)
	eq(t, x.Pending, 0)
}

func TestNewReposIgnoreIsSilent(t *testing.T) {
	h := newHarness(t, func(c *config.Config) { c.Filter.NewRepos = "ignore" })
	h.src.set(mk(1, "site"))
	h.run(false)
	_, has := h.e.state.get(1)
	isTrue(t, !has, "ignored repos leave no trace")
	isTrue(t, !h.tgt.has("alice-gh/site"), "not synced")
}

func TestExistingReposAreGrandfathered(t *testing.T) {
	h := newHarness(t, nil)
	h.src.set(mk(1, "site"))
	h.run(false)
	h.src.set(mk(2, "fresh"))
	h.rebuild(func(c *config.Config) { c.Filter.NewRepos = "review" })
	h.src.set(mk(1, "site", upd("2027-01-01T00:00:00Z")))
	h.run(false)
	isTrue(t, h.tgt.has("alice-gh/site"), "already synced repo keeps syncing")
	isTrue(t, !h.tgt.has("alice-gh/fresh"), "repo that appeared later waits")
	eq(t, h.e.Decide(mk(2, "fresh")).Pending, true)
}

func TestExplicitDecisionBeatsFilters(t *testing.T) {
	r := mk(1, "site", func(r *forge.Repo) { r.Fork = true })
	h := newHarness(t, func(c *config.Config) {
		c.Filter.SkipForks = true
		withRules(map[string]*config.RepoRule{"alice/site": {Sync: yes()}})(c)
	})
	isTrue(t, h.e.Decide(r).Sync, "explicit allow wins over skip_forks")
	h = newHarness(t, withRules(map[string]*config.RepoRule{"alice/site": {Sync: no()}}))
	v := h.e.Decide(mk(1, "site"))
	isTrue(t, !v.Sync && !v.Pending && strings.Contains(v.Why, "ignored"), "explicit ignore")
}

func TestScopeAdmin(t *testing.T) {
	h := newHarness(t, func(c *config.Config) { c.Filter.Scope = "admin" })
	isTrue(t, h.e.Decide(mk(1, "a", func(r *forge.Repo) { r.Role = forge.RoleAdmin })).Sync, "admin")
	isTrue(t, !h.e.Decide(mk(2, "b", func(r *forge.Repo) { r.Role = forge.RoleWrite })).Sync, "collaborator")
	isTrue(t, h.e.Decide(mk(3, "c")).Sync, "unknown role is not held against the repo")
}

func TestPerRepoOverrides(t *testing.T) {
	h := newHarness(t, withRules(map[string]*config.RepoRule{
		"alice/site": {Name: "site-mirror", KeepPrivate: yes(), Metadata: no()},
	}))
	h.src.set(mk(1, "site", func(r *forge.Repo) {
		r.Private = false
		r.Description = "hello"
		r.Topics = []string{"go"}
	}))
	h.run(false)
	g := h.tgt.repos["alice-gh/site-mirror"]
	isTrue(t, g != nil, "GitHub name override")
	isTrue(t, g.Private, "keep_private holds the copy private")
	eq(t, *g.Description, "")
	eq(t, len(g.Topics), 0)
}

func TestPerRepoOnDelete(t *testing.T) {
	h := newHarness(t, withRules(map[string]*config.RepoRule{"alice/site": {OnDelete: "ignore"}}))
	h.src.set(mk(1, "site"))
	h.run(false)
	delete(h.src.repos, 1)
	h.run(false)
	isTrue(t, h.tgt.has("alice-gh/site"), "on_delete = ignore keeps the copy")
}

func TestPruneOptionReachesTheMirror(t *testing.T) {
	h := newHarness(t, withRules(map[string]*config.RepoRule{"alice/site": {Prune: no()}}))
	h.src.set(mk(1, "site"))
	h.src.set(mk(2, "other"))
	h.run(false)
	for i, c := range h.mir.calls {
		prune := h.mir.opts[i].Prune
		if strings.Contains(c, "alice/site") && prune {
			t.Fatal("prune = false must be passed to the mirror")
		}
		if strings.Contains(c, "alice/other") && !prune {
			t.Fatal("prune stays on for repos without the option")
		}
	}
	eq(t, len(h.mir.calls), 2)
}
