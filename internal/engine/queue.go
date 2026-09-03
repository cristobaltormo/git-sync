package engine

import (
	"fmt"
	"runtime/debug"
	"sync"
	"time"

	"github.com/cristobaltormo/git-sync/internal/forge"
	"github.com/cristobaltormo/git-sync/internal/logx"
)

type Job struct {
	Key     string
	Repo    *forge.Repo
	Owner   string
	Name    string
	Why     string
	Verify  bool
	Force   bool
	ReadyAt time.Time
}

type queue struct {
	mu      sync.Mutex
	cond    *sync.Cond
	pending map[string]*Job
	running map[string]bool
	dirty   map[string]*Job
	order   []string
	stopped bool
	wg      sync.WaitGroup
}

func (q *queue) init() {
	q.cond = sync.NewCond(&q.mu)
	q.pending = map[string]*Job{}
	q.running = map[string]bool{}
	q.dirty = map[string]*Job{}
}

func (e *Engine) Enqueue(j *Job) {
	q := &e.queue
	q.mu.Lock()
	defer q.mu.Unlock()
	if q.running[j.Key] {
		if old := q.dirty[j.Key]; old != nil {
			j.Verify = j.Verify || old.Verify
		}
		q.dirty[j.Key] = j
		return
	}
	if old, ok := q.pending[j.Key]; ok {
		old.Verify = old.Verify || j.Verify
		if j.Repo != nil {
			old.Repo = j.Repo
		}
		return
	}
	q.pending[j.Key] = j
	q.order = append(q.order, j.Key)
	q.cond.Signal()
}

func (e *Engine) Start() {
	for i := 0; i < e.Config().Sync.Workers; i++ {
		e.queue.wg.Add(1)
		go e.worker()
	}
}

func (q *queue) next() (string, *Job, bool) {
	for {
		if q.stopped {
			return "", nil, false
		}
		now := time.Now()
		var soonest time.Duration
		for i, k := range q.order {
			j := q.pending[k]
			if !j.ReadyAt.After(now) {
				q.order = append(q.order[:i], q.order[i+1:]...)
				delete(q.pending, k)
				q.running[k] = true
				return k, j, true
			}
			if d := j.ReadyAt.Sub(now); soonest == 0 || d < soonest {
				soonest = d
			}
		}
		if soonest > 0 {
			time.AfterFunc(soonest, q.cond.Broadcast)
		}
		q.cond.Wait()
	}
}

func (e *Engine) worker() {
	q := &e.queue
	defer q.wg.Done()
	for {
		q.mu.Lock()
		k, job, ok := q.next()
		q.mu.Unlock()
		if !ok {
			return
		}
		e.execute(job)
		q.mu.Lock()
		delete(q.running, k)
		if again, ok := q.dirty[k]; ok {
			delete(q.dirty, k)
			q.pending[k] = again
			q.order = append(q.order, k)
			q.cond.Signal()
		}
		idle := len(q.order) == 0 && len(q.running) == 0
		q.mu.Unlock()
		if idle {
			debug.FreeOSMemory()
		}
	}
}

func (e *Engine) execute(job *Job) {
	defer func() {
		if r := recover(); r != nil {
			e.recordFailure(job, fmt.Errorf("internal error: %v", r))
		}
	}()
	if err := e.runJob(job); err != nil {
		e.recordFailure(job, err)
	}
}

func (e *Engine) Drain(timeout time.Duration) bool {
	q := &e.queue
	end := time.Now().Add(timeout)
	for {
		q.mu.Lock()
		busy := len(q.pending) > 0 || len(q.running) > 0
		q.mu.Unlock()
		if !busy {
			return true
		}
		if timeout > 0 && time.Now().After(end) {
			return false
		}
		time.Sleep(20 * time.Millisecond)
	}
}

func (e *Engine) Shutdown() {
	q := &e.queue
	q.mu.Lock()
	q.stopped = true
	q.cond.Broadcast()
	q.mu.Unlock()
	done := make(chan struct{})
	go func() { q.wg.Wait(); close(done) }()
	select {
	case <-done:
	case <-time.After(60 * time.Second):
		logx.Warnf("some jobs were still running at shutdown")
	}
	e.state.save()
}

func (e *Engine) queued() int {
	e.queue.mu.Lock()
	defer e.queue.mu.Unlock()
	return len(e.queue.order)
}
