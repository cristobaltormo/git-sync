package cli

import (
	"path"
	"sort"
	"strings"

	"github.com/cristobaltormo/git-sync/internal/config"
	"github.com/cristobaltormo/git-sync/internal/engine"
	"github.com/cristobaltormo/git-sync/internal/forge"
)

const (
	stSynced   = "synced"
	stError    = "error"
	stQueued   = "queued"
	stWaiting  = "waiting"
	stIgnored  = "ignored"
	stFiltered = "filtered"
)

type item struct {
	Repo    *forge.Repo
	Verdict engine.Verdict
	Entry   engine.Entry
	Has     bool
	Status  string
}

func (it *item) full() string { return it.Repo.Full() }

func (it *item) managed() bool { return it.Has && it.Entry.Managed }

func (it *item) target() string {
	if it.Has && it.Entry.GHOwner != "" {
		return it.Entry.GHOwner + "/" + it.Entry.GHName
	}
	return ""
}

func classify(v engine.Verdict, x engine.Entry, has bool) string {
	switch {
	case v.Pending:
		return stWaiting
	case v.Sync && has && x.Managed && x.LastError != "":
		return stError
	case v.Sync && has && x.Managed:
		return stSynced
	case v.Sync:
		return stQueued
	case v.Rule != nil && v.Rule.Sync != nil:
		return stIgnored
	}
	return stFiltered
}

func inventory(a *app) ([]*item, error) {
	repos, err := a.eng.ListAll()
	if err != nil {
		return nil, err
	}
	entries := a.eng.Entries()
	out := make([]*item, 0, len(repos))
	for _, r := range repos {
		v := a.eng.Decide(r)
		x, has := entries[r.ID]
		out = append(out, &item{Repo: r, Verdict: v, Entry: x, Has: has, Status: classify(v, x, has)})
	}
	sort.Slice(out, func(i, j int) bool {
		return strings.ToLower(out[i].full()) < strings.ToLower(out[j].full())
	})
	return out, nil
}

func counts(items []*item) map[string]int {
	c := map[string]int{}
	for _, it := range items {
		c[it.Status]++
	}
	return c
}

func match(items []*item, pattern string) []*item {
	p := strings.ToLower(pattern)
	var out []*item
	for _, it := range items {
		full, name := strings.ToLower(it.full()), strings.ToLower(it.Repo.Name)
		ok, _ := path.Match(p, full)
		if !ok && !strings.Contains(p, "/") {
			ok, _ = path.Match(p, name)
		}
		if ok {
			out = append(out, it)
		}
	}
	return out
}

func resolve(items []*item, patterns []string) ([]*item, error) {
	seen := map[int64]bool{}
	var out []*item
	for _, p := range patterns {
		found := match(items, p)
		if len(found) == 0 {
			return nil, config.Errorf("no repository matches %q (see `gitsync repos list`)", p)
		}
		for _, it := range found {
			if !seen[it.Repo.ID] {
				seen[it.Repo.ID] = true
				out = append(out, it)
			}
		}
	}
	return out, nil
}
