package engine

import (
	"time"

	"github.com/cristobaltormo/git-sync/internal/forge"
	"github.com/cristobaltormo/git-sync/internal/logx"
)

func (e *Engine) HookMode() string {
	e.hooksMu.Lock()
	defer e.hooksMu.Unlock()
	return e.hooksMode
}

func (e *Engine) setHookMode(m string) {
	e.hooksMu.Lock()
	e.hooksMode = m
	e.hooksMu.Unlock()
}

// false means the source was unreachable and setup should be retried
func (e *Engine) SetupHooks(force bool) bool {
	mode := e.Config().Hooks.Mode
	h := e.src.Hooks()
	if mode == "none" || e.dry || h == nil {
		e.setHookMode("none")
		return true
	}
	me, err := e.src.Whoami()
	if err != nil {
		logx.Errorf("cannot set up webhooks yet, the source is not reachable: %v", err)
		return false
	}
	if mode == "system" || (mode == "auto" && me.SystemHooks) {
		id, res, err := h.Global(e.state.systemHook(), force)
		if err == nil {
			e.state.setSystemHook(id)
			e.setHookMode("system")
			logx.Infof("system webhook %s (covers every repo, new ones included)", res)
			return true
		}
		if mode == "system" {
			logx.Errorf("cannot create the system webhook: %v", err)
			return false
		}
		logx.Warnf("system webhook failed (%v), falling back to per-repo webhooks", err)
	}
	e.setHookMode("repo")
	logx.Infof("using per-repo webhooks (created as each repo is synced)")
	return true
}

func (e *Engine) HandleEvent(ev *forge.Event) {
	if _, mapped := e.Config().GitHubOwner(ev.Owner); !mapped {
		return
	}
	if ev.Kind == forge.Removed {
		e.Wake()
		return
	}
	k := key(ev.ID)
	if ev.ID == 0 {
		k = ev.Owner + "/" + ev.Name
	}
	e.Enqueue(&Job{Key: k, Owner: ev.Owner, Name: ev.Name, Why: "webhook", ReadyAt: time.Now().Add(hookDelay)})
}

// creating a branch sends a push and a create webhook within milliseconds
const hookDelay = 100 * time.Millisecond

func (e *Engine) repoHook(repo *forge.Repo, have string) {
	url := e.Config().HookURL()
	h := e.src.Hooks()
	if have == url || e.dry || h == nil {
		return
	}
	if _, err := h.Repo(repo, false); err != nil {
		logx.Warnf("%s: could not create the webhook: %v", repo.Full(), err)
		return
	}
	e.state.update(repo.ID, false, func(x *Entry) { x.Hook = url })
}
