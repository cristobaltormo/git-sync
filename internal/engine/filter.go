package engine

import (
	"path"
	"slices"
	"strings"

	"github.com/cristobaltormo/git-sync/internal/forge"
)

func (e *Engine) Selected(r *forge.Repo) (bool, string) {
	f := e.Config().Filter
	switch {
	case f.SkipForks && r.Fork:
		return false, "fork"
	case f.SkipMirrors && r.Mirror:
		return false, "pull mirror"
	case f.SkipArchived && r.Archived:
		return false, "archived"
	case f.SkipPrivate && r.Private:
		return false, "private"
	case f.Topic != "" && !slices.Contains(r.Topics, f.Topic):
		return false, "no '" + f.Topic + "' topic"
	}
	if len(f.Include) > 0 && !matches(f.Include, r) {
		return false, "not in filter.include"
	}
	if matches(f.Exclude, r) {
		return false, "in filter.exclude"
	}
	return true, ""
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
