package engine

import (
	"crypto/sha1"
	"encoding/hex"
	"fmt"
	"sort"
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

// built from the listing: Forgejo bumps updated_at on every edit, Gitea only on some
func fingerprint(repos []*forge.Repo) string {
	sorted := append([]*forge.Repo(nil), repos...)
	sort.Slice(sorted, func(i, j int) bool { return sorted[i].ID < sorted[j].ID })
	h := sha1.New()
	for _, r := range sorted {
		fmt.Fprintf(h, "%d|%s|%s|%t|%t|%t\n", r.ID, r.Sig(), r.Updated, r.Empty, r.Mirror, r.Fork)
	}
	return hex.EncodeToString(h.Sum(nil))
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
		v := e.Decide(repo)
		if v.Pending {
			e.markPending(repo, ent, has)
			continue
		}
		if has && ent.Pending != 0 {
			e.state.update(repo.ID, false, func(x *Entry) { x.Pending = 0 })
			e.state.save()
		}
		if !v.Sync {
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

func (e *Engine) markPending(repo *forge.Repo, ent Entry, has bool) {
	if has && ent.Pending != 0 {
		return
	}
	now := time.Now().Unix()
	e.state.update(repo.ID, true, func(x *Entry) {
		x.Owner, x.Name, x.Pending = repo.Owner, repo.Name, now
	})
	e.state.save()
	logx.Infof("%s: new repository, not synced until you decide (run `gitsync repos`)", repo.Full())
	e.notify(Event{Kind: EventPending, Repo: repo.Full()})
}

func (e *Engine) nextRetry() (time.Time, bool) {
	var best int64
	for _, x := range e.state.all() {
		if x.LastError != "" && x.RetryAt > 0 && (best == 0 || x.RetryAt < best) {
			best = x.RetryAt
		}
	}
	if best == 0 {
		return time.Time{}, false
	}
	return time.Unix(best, 0), true
}

func (e *Engine) HasMissing() bool {
	for _, x := range e.state.all() {
		if x.MissingSince != 0 {
			return true
		}
	}
	return false
}
