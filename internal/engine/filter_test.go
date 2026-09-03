package engine

import (
	"testing"

	"github.com/cristobaltormo/git-sync/internal/config"
	"github.com/cristobaltormo/git-sync/internal/forge"
)

func TestFilters(t *testing.T) {
	sel := func(mod func(*config.Config), r *forge.Repo) bool {
		h := newHarness(t, mod)
		ok, _ := h.e.Selected(r)
		return ok
	}
	site := func(m ...func(*forge.Repo)) *forge.Repo { return mk(1, "site", m...) }
	isTrue(t, sel(nil, site()), "default")
	isTrue(t, !sel(nil, site(func(r *forge.Repo) { r.Mirror = true })), "pull mirrors skipped")
	isTrue(t, sel(func(c *config.Config) { c.Filter.SkipMirrors = false }, site(func(r *forge.Repo) { r.Mirror = true })), "mirrors allowed")
	isTrue(t, !sel(func(c *config.Config) { c.Filter.SkipForks = true }, site(func(r *forge.Repo) { r.Fork = true })), "forks")
	isTrue(t, !sel(func(c *config.Config) { c.Filter.SkipArchived = true }, site(func(r *forge.Repo) { r.Archived = true })), "archived")
	isTrue(t, !sel(func(c *config.Config) { c.Filter.SkipPrivate = true }, site()), "private")
	isTrue(t, !sel(func(c *config.Config) { c.Filter.Exclude = []string{"si*"} }, site()), "exclude glob")
	isTrue(t, !sel(func(c *config.Config) { c.Filter.Exclude = []string{"alice/site"} }, site()), "exclude full")
	isTrue(t, sel(func(c *config.Config) { c.Filter.Include = []string{"alice/*"} }, site()), "include")
	isTrue(t, !sel(func(c *config.Config) { c.Filter.Include = []string{"bob/*"} }, site()), "include miss")
	isTrue(t, !sel(func(c *config.Config) { c.Filter.Topic = "mirror" }, site()), "topic missing")
	isTrue(t, sel(func(c *config.Config) { c.Filter.Topic = "mirror" }, site(func(r *forge.Repo) { r.Topics = []string{"mirror"} })), "topic")
}

func TestExcludedIsLeftAlone(t *testing.T) {
	h := newHarness(t, nil)
	h.src.set(mk(1, "site"))
	h.run(false)
	h.rebuild(func(c *config.Config) { c.Filter.Exclude = []string{"site"} })
	h.run(false)
	x, _ := h.e.state.get(1)
	isTrue(t, x.Excluded, "should be marked excluded")
	delete(h.src.repos, 1)
	h.tgt.calls = nil
	h.run(false)
	isTrue(t, h.tgt.has("alice-gh/site"), "excluded repo must not be deleted")
}
