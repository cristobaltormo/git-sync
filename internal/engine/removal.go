package engine

import (
	"strings"
	"time"

	"github.com/cristobaltormo/git-sync/internal/logx"
)

func (e *Engine) handleMissing(id int64, x Entry) {
	if !x.Managed {
		e.state.drop(id)
		e.state.save()
		return
	}
	now := time.Now().Unix()
	first := x.MissingSince
	if first == 0 {
		first = now
		e.state.update(id, false, func(y *Entry) { y.MissingSince = now })
	}
	grace := int64(e.Config().Sync.DeleteGrace)
	if now-first < grace {
		logx.Infof("%s/%s is missing at the source, waiting %ds before touching GitHub", x.Owner, x.Name, grace)
		return
	}
	// a token that lost access looks like a deleted repo, so ask for it directly
	still, err := e.src.Get(x.Owner, x.Name)
	if err != nil {
		logx.Warnf("%s/%s: cannot confirm the removal (%v), leaving it", x.Owner, x.Name, err)
		return
	}
	if still != nil {
		e.state.update(id, false, func(y *Entry) { y.MissingSince = 0 })
		return
	}
	if ok, err := e.src.OwnerExists(x.Owner); err != nil || !ok {
		logx.Warnf("%s/%s: owner lookup failed, not treating it as deleted", x.Owner, x.Name)
		return
	}
	e.applyRemoval(id, x)
}

func (e *Engine) withinDeleteLimit(limit int) bool {
	e.delMu.Lock()
	defer e.delMu.Unlock()
	now := time.Now()
	for len(e.deletions) > 0 && now.Sub(e.deletions[0]) > time.Hour {
		e.deletions = e.deletions[1:]
	}
	return limit == 0 || len(e.deletions) < limit
}

func (e *Engine) applyRemoval(id int64, x Entry) {
	sy := e.Config().Sync
	name := x.GHOwner + "/" + x.GHName
	switch sy.OnDelete {
	case "delete":
		if !e.withinDeleteLimit(sy.DeleteLimit) {
			logx.Errorf("refusing to delete %s: %d repos were already deleted in the last hour (sync.delete_limit). "+
				"If that is expected, restart to reset it.", name, sy.DeleteLimit)
			return
		}
		g, err := e.tgt.Get(x.GHOwner, x.GHName)
		if err != nil {
			logx.Errorf("%s: %v", name, err)
			return
		}
		switch {
		case g == nil:
			logx.Infof("%s: already gone from GitHub", name)
		case !strings.EqualFold(g.Owner.Login, x.GHOwner):
			logx.Warnf("%s: owned by someone else on GitHub, not deleting", name)
		default:
			if err := e.tgt.Delete(x.GHOwner, x.GHName); err != nil {
				logx.Errorf("%s: %v", name, err)
				return
			}
			e.delMu.Lock()
			e.deletions = append(e.deletions, time.Now())
			e.delMu.Unlock()
			logx.Infof("%s: deleted on GitHub (removed from the source)", name)
		}
	case "archive":
		g, err := e.tgt.Get(x.GHOwner, x.GHName)
		if err != nil {
			logx.Errorf("%s: %v", name, err)
			return
		}
		if g != nil && !g.Archived {
			if err := e.tgt.Patch(x.GHOwner, x.GHName, map[string]any{"archived": true}); err != nil {
				logx.Errorf("%s: %v", name, err)
				return
			}
			logx.Infof("%s: archived on GitHub (removed from the source)", name)
		}
	default:
		logx.Infof("%s: removed from the source, the GitHub copy is left alone (on_delete = ignore)", name)
	}
	e.state.drop(id)
	e.mir.Forget(id)
	e.state.save()
}
