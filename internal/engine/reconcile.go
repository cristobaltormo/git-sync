package engine

import (
	"time"

	"github.com/cristobaltormo/git-sync/internal/forge"
	"github.com/cristobaltormo/git-sync/internal/logx"
)

func (e *Engine) ListAll() ([]*forge.Repo, error) {
	var all []*forge.Repo
	for owner := range e.Config().Accounts {
		repos, err := e.src.List(owner)
		if err != nil {
			return nil, err
		}
		all = append(all, repos...)
	}
	return all, nil
}

func (e *Engine) Reconcile(verify bool, why string) bool {
	repos, err := e.ListAll()
	if err != nil {
		logx.Errorf("listing repos failed, skipping this scan: %v", err)
		return false
	}
	e.reconcileRepos(repos, verify, why)
	return true
}

func (e *Engine) reconcileRepos(repos []*forge.Repo, verify bool, why string) {
	now := time.Now().Unix()
	seen := map[int64]bool{}
	for _, repo := range repos {
		seen[repo.ID] = true
		ent, has := e.state.get(repo.ID)
		if ok, _ := e.Selected(repo); !ok {
			if has && ent.Managed && !ent.Excluded {
				e.Enqueue(&Job{Key: key(repo.ID), Repo: repo, Why: why})
			}
			continue
		}
		if has && ent.RetryAt > now {
			continue
		}
		due := !has || !ent.Managed || ent.Excluded || ent.Sig != repo.Sig() ||
			ent.Updated != repo.Updated || ent.LastError != ""
		if due || verify {
			w := why
			if !due {
				w = "verify"
			}
			e.Enqueue(&Job{Key: key(repo.ID), Repo: repo, Why: w, Verify: verify})
		}
	}
	entries := e.state.all()
	managed := 0
	for _, x := range entries {
		if x.Managed {
			managed++
		}
	}
	if len(repos) == 0 && managed > 1 {
		logx.Warnf("the source returned no repos at all; not treating that as a mass deletion")
		return
	}
	for id, x := range entries {
		if !seen[id] && !x.Excluded {
			e.handleMissing(id, x)
		}
	}
}

func (e *Engine) HasMissing() bool {
	for _, x := range e.state.all() {
		if x.MissingSince != 0 {
			return true
		}
	}
	return false
}
