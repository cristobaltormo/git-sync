package engine

import (
	"path"
	"slices"
	"strings"

	"github.com/cristobaltormo/git-sync/internal/config"
	"github.com/cristobaltormo/git-sync/internal/forge"
)

type Verdict struct {
	Sync    bool
	Pending bool
	Why     string
	Rule    *config.RepoRule
}

func (e *Engine) Selected(r *forge.Repo) (bool, string) {
	v := e.Decide(r)
	return v.Sync, v.Why
}

func (e *Engine) Decide(r *forge.Repo) Verdict {
	cfg := e.Config()
	rule := cfg.Repos.Rule(r.Full())
	if rule != nil && rule.Sync != nil {
		if *rule.Sync {
			return Verdict{Sync: true, Why: "allowed by you", Rule: rule}
		}
		return Verdict{Why: "ignored by you", Rule: rule}
	}
	f := cfg.Filter
	no := func(why string) Verdict { return Verdict{Why: why, Rule: rule} }
	switch {
	case f.SkipForks && r.Fork:
		return no("fork")
	case f.SkipMirrors && r.Mirror:
		return no("pull mirror")
	case f.SkipArchived && r.Archived:
		return no("archived")
	case f.SkipPrivate && r.Private:
		return no("private")
	case f.Topic != "" && !slices.Contains(r.Topics, f.Topic):
		return no("no '" + f.Topic + "' topic")
	case f.Scope == "admin" && r.Role != "" && r.Role != forge.RoleAdmin:
		return no("you are not an admin of it")
	}
	if len(f.Include) > 0 && !matches(f.Include, r) {
		return no("not in filter.include")
	}
	if matches(f.Exclude, r) {
		return no("in filter.exclude")
	}
	if f.NewRepos != "sync" {
		if x, has := e.state.get(r.ID); !has || !x.Managed {
			if f.NewRepos == "review" {
				return Verdict{Pending: true, Why: "waiting for your decision", Rule: rule}
			}
			return no("not allowed yet")
		}
	}
	return Verdict{Sync: true, Rule: rule}
}

func matches(patterns []string, r *forge.Repo) bool {
	full, name := strings.ToLower(r.Full()), strings.ToLower(r.Name)
	for _, p := range patterns {
		p = strings.ToLower(p)
		if ok, _ := path.Match(p, full); ok {
			return true
		}
		if ok, _ := path.Match(p, name); ok && !strings.Contains(p, "/") {
			return true
		}
	}
	return false
}
