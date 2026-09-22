package engine

import (
	"fmt"
	"strings"
	"time"

	"github.com/cristobaltormo/git-sync/internal/config"
	"github.com/cristobaltormo/git-sync/internal/forge"
	"github.com/cristobaltormo/git-sync/internal/github"
	"github.com/cristobaltormo/git-sync/internal/logx"
)

type repoSync struct {
	e       *Engine
	cfg     *config.Config
	repo    *forge.Repo
	job     *Job
	owner   string
	name    string
	ent     Entry
	has     bool
	managed bool
	cur     *github.Snap
	actions []string
}

func (e *Engine) syncRepo(repo *forge.Repo, job *Job) (err error) {
	cfg := e.Config()
	owner, ok := cfg.GitHubOwner(repo.Owner)
	if !ok {
		return nil
	}
	s := &repoSync{e: e, cfg: cfg, repo: repo, job: job, owner: owner, name: repo.Name}
	s.ent, s.has = e.state.get(repo.ID)
	s.managed = s.has && s.ent.Managed

	if err := s.rename(); err != nil {
		return err
	}
	if err := s.resolve(); err != nil {
		return err
	}
	e.state.update(repo.ID, true, func(x *Entry) {
		x.Owner, x.Name, x.GHOwner, x.GHName = repo.Owner, repo.Name, s.owner, s.name
		x.Managed, x.Excluded, x.MissingSince = true, false, 0
	})
	defer func() {
		snap := s.cur.Clone()
		e.state.update(repo.ID, false, func(x *Entry) { x.GH = &snap })
		if err != nil {
			e.state.save()
		}
	}()

	skip := s.skipPush()
	if err := s.beforePush(skip); err != nil {
		return err
	}
	if !skip {
		if err := s.push(); err != nil {
			return err
		}
	}
	if err := s.afterPush(skip); err != nil {
		return err
	}
	if e.HookMode() == "repo" {
		e.repoHook(repo, s.ent.Hook)
	}
	s.finish()
	return nil
}

func (s *repoSync) note(format string, a ...any) {
	s.actions = append(s.actions, fmt.Sprintf(format, a...))
}

func (s *repoSync) resolve() error {
	sy := s.cfg.Sync
	if s.managed && s.ent.GH != nil && !s.job.Verify {
		c := s.ent.GH.Clone()
		s.cur = &c
		return nil
	}
	data, err := s.e.tgt.Get(s.owner, s.name)
	if err != nil {
		return err
	}
	if data != nil {
		snap := github.Snapshot(data)
		s.cur = &snap
		if !s.managed {
			if !sy.AdoptExisting {
				return &blockedError{fmt.Sprintf(
					"%s/%s already exists on GitHub and gitsync did not create it; set sync.adopt_existing = true to take it over",
					s.owner, s.name)}
			}
			s.note("adopted existing GitHub repo")
		}
		return nil
	}
	desc, home := "", ""
	if sy.Metadata {
		desc, home = describeRepo(s.repo), homepage(s.repo)
	}
	created, err := s.e.tgt.Create(s.owner, s.name, desc, home)
	if err != nil {
		return err
	}
	snap := github.Snap{Private: true, Description: desc, Homepage: home}
	if created != nil {
		snap = github.Snapshot(created)
	}
	s.cur = &snap
	s.note("created on GitHub (private)")
	return nil
}

func (s *repoSync) skipPush() bool {
	sy := s.cfg.Sync
	unchanged := s.managed && s.repo.Updated != "" && s.ent.Updated == s.repo.Updated && s.ent.LastError == ""
	recent := s.ent.LastPush > 0 && time.Since(time.Unix(s.ent.LastPush, 0)) < 24*time.Hour
	return s.repo.Empty ||
		(s.cur.Archived && !sy.Archived) ||
		(s.cur.Archived && s.repo.Archived && unchanged) ||
		(s.job.Verify && !s.job.Force && unchanged && recent)
}

func (s *repoSync) push() error {
	last := ""
	if !s.job.Verify && !s.job.Force {
		last = s.ent.Refs
	}
	res, err := s.e.mir.Mirror(s.repo, s.owner, s.name, last)
	if err != nil {
		return err
	}
	s.e.state.update(s.repo.ID, false, func(x *Entry) { x.LastPush, x.Refs = time.Now().Unix(), res.Refs })
	if res.Summary != "up to date" {
		s.actions = append([]string{"pushed (" + res.Summary + ")"}, s.actions...)
	}
	return nil
}

func (s *repoSync) finish() {
	now := time.Now().Unix()
	snap := s.cur.Clone()
	s.e.state.update(s.repo.ID, false, func(x *Entry) {
		x.Sig, x.Updated, x.GH = s.repo.Sig(), s.repo.Updated, &snap
		x.LastOK, x.Attempts, x.LastError, x.RetryAt = now, 0, "", 0
	})
	s.e.state.save()
	msg := "nothing to do"
	if len(s.actions) > 0 {
		msg = strings.Join(s.actions, ", ")
	}
	logx.Infof("%s: %s: %s", s.repo.Full(), s.job.Why, msg)
}
