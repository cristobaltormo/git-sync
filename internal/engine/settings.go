package engine

import (
	"errors"
	"fmt"
	"sort"
	"strings"

	"github.com/cristobaltormo/git-sync/internal/config"
	"github.com/cristobaltormo/git-sync/internal/forge"
	"github.com/cristobaltormo/git-sync/internal/github"
	"github.com/cristobaltormo/git-sync/internal/httpx"
	"github.com/cristobaltormo/git-sync/internal/logx"
)

func (s *repoSync) beforePush(skip bool) error {
	pre := map[string]any{}
	if s.cur.Archived && !skip {
		pre["archived"] = false
	}
	if s.cfg.Sync.Visibility && s.repo.Private && !s.cur.Private {
		pre["private"] = true
	}
	if len(pre) == 0 {
		return nil
	}
	if err := s.e.tgt.Patch(s.owner, s.name, pre); err != nil {
		return err
	}
	apply(s.cur, pre)
	s.note("set %s", join(pre))
	return nil
}

func (s *repoSync) afterPush(skipped bool) error {
	sy := s.cfg.Sync
	post := map[string]any{}
	if sy.Visibility && !s.repo.Private && s.cur.Private {
		post["private"] = false
	}
	if sy.Metadata {
		if d := describeRepo(s.repo); d != s.cur.Description {
			post["description"] = d
		}
		if h := homepage(s.repo); h != s.cur.Homepage {
			post["homepage"] = h
		}
	}
	if sy.DefaultBranch && s.repo.DefaultBranch != "" && !s.repo.Empty && s.cur.DefaultBranch != s.repo.DefaultBranch &&
		s.ent.RefusedBranch != s.repo.DefaultBranch {
		post["default_branch"] = s.repo.DefaultBranch
	}
	if len(post) > 0 {
		if err := s.patchPost(post); err != nil {
			return err
		}
	}
	if sy.Metadata {
		topics := github.CleanTopics(s.repo.Topics, s.cfg.Filter.Topic)
		sort.Strings(topics)
		if !equal(topics, s.cur.Topics) {
			if err := s.e.tgt.SetTopics(s.owner, s.name, topics); err != nil {
				return err
			}
			s.cur.Topics = topics
			s.note("updated topics")
		}
	}
	if sy.Archived && s.repo.Archived != s.cur.Archived && (s.repo.Archived || skipped) {
		if err := s.e.tgt.Patch(s.owner, s.name, map[string]any{"archived": s.repo.Archived}); err != nil {
			return err
		}
		s.cur.Archived = s.repo.Archived
		if s.repo.Archived {
			s.note("archived")
		} else {
			s.note("unarchived")
		}
	}
	return nil
}

func (s *repoSync) patchPost(post map[string]any) error {
	err := s.e.tgt.Patch(s.owner, s.name, post)
	var ae *httpx.APIError
	if _, hasBranch := post["default_branch"]; err != nil && hasBranch && errors.As(err, &ae) && ae.Status == 422 {
		logx.Warnf("%s: GitHub refused the default branch '%s', it will not be tried again", s.repo.Full(), s.repo.DefaultBranch)
		branch := s.repo.DefaultBranch
		s.ent.RefusedBranch = branch
		s.e.state.update(s.repo.ID, false, func(x *Entry) { x.RefusedBranch = branch })
		delete(post, "default_branch")
		err = nil
		if len(post) > 0 {
			err = s.e.tgt.Patch(s.owner, s.name, post)
		}
	}
	if err != nil {
		return err
	}
	apply(s.cur, post)
	if len(post) > 0 {
		keys := make([]string, 0, len(post))
		for k := range post {
			keys = append(keys, k)
		}
		sort.Strings(keys)
		s.note("updated %s", strings.Join(keys, ", "))
	}
	return nil
}

func describeRepo(r *forge.Repo) string {
	if len(r.Description) > 350 {
		return r.Description[:347] + "..."
	}
	return r.Description
}

func homepage(r *forge.Repo) string {
	if config.IsHTTPURL(r.Website) {
		return r.Website
	}
	return ""
}

func join(fields map[string]any) string {
	parts := make([]string, 0, len(fields))
	for k, v := range fields {
		parts = append(parts, fmt.Sprintf("%s=%v", k, v))
	}
	sort.Strings(parts)
	return strings.Join(parts, ", ")
}

func apply(s *github.Snap, fields map[string]any) {
	for k, v := range fields {
		switch k {
		case "private":
			s.Private = v.(bool)
		case "archived":
			s.Archived = v.(bool)
		case "description":
			s.Description = v.(string)
		case "homepage":
			s.Homepage = v.(string)
		case "default_branch":
			s.DefaultBranch = v.(string)
		}
	}
}

func equal(a, b []string) bool {
	if len(a) != len(b) {
		return false
	}
	for i := range a {
		if a[i] != b[i] {
			return false
		}
	}
	return true
}
