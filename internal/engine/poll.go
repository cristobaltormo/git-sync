package engine

import (
	"context"
	"time"

	"github.com/cristobaltormo/git-sync/internal/logx"
)

func (e *Engine) PollLoop(ctx context.Context) {
	first, asked, down := true, false, false
	var lastFP string
	var lastHookTry time.Time
	lastVerify := time.Now()
	for ctx.Err() == nil {
		now := time.Now()
		sy := e.Config().Sync
		if e.HookMode() == "" && now.Sub(lastHookTry) >= 30*time.Second {
			lastHookTry = now
			e.SetupHooks(false)
		}

		repos, err := e.ListAll()
		if err != nil {
			if !down {
				logx.Warnf("cannot reach the source, will keep trying quietly: %v", err)
			}
			down = true
		} else {
			fp := fingerprint(repos)
			verify := first || (sy.VerifyInterval > 0 && now.Sub(lastVerify) >= time.Duration(sy.VerifyInterval)*time.Second)
			retryAt, hasRetry := e.nextRetry()
			why := "change"
			switch {
			case first:
				why = "startup"
			case down:
				why = "source is back"
			}
			if first || down || asked || verify || fp != lastFP || e.HasMissing() || (hasRetry && !retryAt.After(now)) {
				e.reconcileRepos(repos, verify, why)
				lastFP = fp
				if verify {
					lastVerify = now
				}
			}
			first, down = false, false
		}
		asked = false

		wait := time.Duration(sy.PollInterval) * time.Second
		if wait == 0 {
			wait = 5 * time.Minute
		}
		if e.HasMissing() {
			wait = min(wait, 5*time.Second)
		}
		if err != nil {
			wait = max(wait, 15*time.Second)
		}
		if at, ok := e.nextRetry(); ok {
			wait = min(wait, max(time.Until(at), time.Second))
		}
		select {
		case <-ctx.Done():
			return
		case <-e.wake:
			asked = true
		case <-time.After(wait):
		}
	}
}
