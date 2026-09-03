package engine

import (
	"errors"
	"strconv"
	"time"

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
	return e.syncRepo(repo, job)
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
		case !isBlocked:
			delay = backoff[min(x.Attempts-1, len(backoff)-1)]
		}
		x.RetryAt = time.Now().Add(delay).Unix()
	})
	switch {
	case isBlocked && attempts > 1:
		logx.Debugf("%s: %v", name, err)
	case isBlocked:
		logx.Warnf("%s: %v", name, err)
	default:
		logx.Errorf("%s: %v", name, err)
	}
	e.state.save()
}
