package engine

import (
	"slices"
	"strings"

	"github.com/cristobaltormo/git-sync/internal/config"
	"github.com/cristobaltormo/git-sync/internal/logx"
)

const defaultPullMessage = "This repository is a read-only mirror. Whatever is merged here is overwritten by the next sync, so this pull request cannot be merged on GitHub. Please send it to the maintainer instead."

func (e *Engine) checkPulls() {
	cfg := e.Config()
	comment := cfg.Pulls.Mode == "comment"
	tell := cfg.Notify.URL != "" && slices.Contains(cfg.Notify.Events, EventPullRequest)
	if (!comment && !tell) || e.dry {
		return
	}
	managed := map[string]string{}
	for _, x := range e.state.all() {
		if x.Managed && x.GHOwner != "" {
			managed[strings.ToLower(x.GHOwner+"/"+x.GHName)] = x.Owner + "/" + x.Name
		}
	}
	owners := map[string]bool{}
	for _, gh := range cfg.Accounts {
		owners[gh] = true
	}
	open := map[string]bool{}
	complete := true
	known, ready := e.state.pulls()
	for owner := range owners {
		pulls, err := e.tgt.OpenPulls(owner)
		if err != nil {
			logx.Warnf("cannot look for pull requests of %s: %v", owner, err)
			complete = false
			continue
		}
		for _, p := range pulls {
			src, ok := managed[strings.ToLower(p.Owner+"/"+p.Repo)]
			if !ok {
				continue
			}
			key := p.Key()
			open[key] = true
			if _, seen := known[key]; seen || !ready {
				e.state.markPull(key)
				continue
			}
			e.state.markPull(key)
			if comment {
				if err := e.tgt.Comment(p.Owner, p.Repo, p.Number, pullMessage(cfg, src)); err != nil {
					logx.Warnf("%s: could not comment: %v", key, err)
				} else {
					logx.Infof("%s: pull request on a mirror, left a note (it is never merged or closed by gitsync)", key)
				}
			}
			e.notify(Event{Kind: EventPullRequest, Repo: src, Info: p.URL})
		}
	}
	e.state.finishPulls(open, complete)
	e.state.save()
}

func pullMessage(cfg *config.Config, src string) string {
	msg := cfg.Pulls.Message
	if msg == "" {
		return defaultPullMessage
	}
	r := strings.NewReplacer("{repo}", src, "{url}", cfg.SourceURL()+"/"+src)
	return r.Replace(msg)
}
