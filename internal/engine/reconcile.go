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
	for _, repo := range repos {
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
}
