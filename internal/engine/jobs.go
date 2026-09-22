package engine

import (
	"errors"
	"strconv"
	"strings"
	"time"

	"github.com/cristobaltormo/git-sync/internal/forge"
	"github.com/cristobaltormo/git-sync/internal/httpx"
	"github.com/cristobaltormo/git-sync/internal/logx"
)

var backoff = []time.Duration{30 * time.Second, 2 * time.Minute, 10 * time.Minute, 30 * time.Minute}

type blockedError struct{ msg string }

func (b *blockedError) Error() string { return b.msg }

func (e *Engine) runJob(job *Job) error {
	repo := job.Repo
	if repo == nil {
		r, err := e.src.Get(job.Owner, job.Name)
		if err != nil {
			return err
		}
		if r == nil {
			logx.Infof("%s/%s is gone from the source, checking", job.Owner, job.Name)
			e.Wake()
			return nil
		}
		repo = r
	}
	if ok, why := e.Selected(repo); !ok {
		if x, has := e.state.get(repo.ID); has && !x.Excluded {
			e.state.update(repo.ID, false, func(x *Entry) { x.Excluded = true })
			e.state.save()
			logx.Infof("%s: no longer synced (%s); the GitHub copy is left alone", repo.Full(), why)
		}
		return nil
	}
	err := e.syncRepo(repo, job)
	if err != nil && e.vanished(repo) {
		logx.Infof("%s was removed from the source while it was being synced", repo.Full())
		e.Wake()
		return nil
	}
	return err
}

func (e *Engine) vanished(r *forge.Repo) bool {
	got, err := e.src.Get(r.Owner, r.Name)
	return err == nil && got == nil
}

// transient answers while GitHub applies a visibility change
func isBusy(err error) bool {
	var ae *httpx.APIError
	if errors.As(err, &ae) && ae.Status == 422 && strings.Contains(strings.ToLower(ae.Msg), "in progress") {
		return true
	}
	return strings.Contains(err.Error(), "is disabled")
}

func (e *Engine) recordFailure(job *Job, err error) {
	var id int64
	name := job.Owner + "/" + job.Name
	if job.Repo != nil {
		id, name = job.Repo.ID, job.Repo.Full()
	} else if n, perr := strconv.ParseInt(job.Key, 10, 64); perr == nil {
		id = n
	}
	var blocked *blockedError
	isBlocked := errors.As(err, &blocked)
	busy := isBusy(err)
	if id == 0 {
		logx.Errorf("%s: %v", name, err)
		return
	}
	var attempts int
	e.state.update(id, job.Repo != nil, func(x *Entry) {
		if x.Owner == "" && job.Repo != nil {
			x.Owner, x.Name = job.Repo.Owner, job.Repo.Name
		}
		x.LastError = err.Error()
		x.Attempts++
		attempts = x.Attempts
		delay := 10 * time.Minute
		switch {
		case busy:
			delay = min(10*time.Second<<min(x.Attempts-1, 3), time.Minute)
		case !isBlocked:
			delay = backoff[min(x.Attempts-1, len(backoff)-1)]
		}
		x.RetryAt = time.Now().Add(delay).Unix()
	})
	switch {
	case busy:
		logx.Warnf("%s: GitHub is still busy with a previous change, will retry shortly (%v)", name, err)
	case isBlocked && attempts > 1:
		logx.Debugf("%s: %v", name, err)
	case isBlocked:
		logx.Warnf("%s: %v", name, err)
	default:
		logx.Errorf("%s: %v", name, err)
	}
	e.state.save()
}
