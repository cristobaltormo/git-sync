package engine

import (
	"sync"
	"time"

	"github.com/cristobaltormo/git-sync/internal/config"
	"github.com/cristobaltormo/git-sync/internal/forge"
	"github.com/cristobaltormo/git-sync/internal/github"
	"github.com/cristobaltormo/git-sync/internal/mirror"
)

type Source interface {
	Kind() string
	Whoami() (*forge.Identity, error)
	List(owner string) ([]*forge.Repo, error)
	Get(owner, name string) (*forge.Repo, error)
	OwnerExists(owner string) (bool, error)
	Hooks() forge.Hooks
}

type Target interface {
	Get(owner, name string) (*github.Repo, error)
	Create(owner, name, description, homepage string) (*github.Repo, error)
	Patch(owner, name string, fields map[string]any) error
	SetTopics(owner, name string, topics []string) error
	Delete(owner, name string) error
}

type Mirror interface {
	Mirror(r *forge.Repo, ghOwner, ghName, last string) (mirror.Result, error)
	Forget(id int64)
}

type Engine struct {
	cfgMu sync.RWMutex
	cfg   *config.Config

	src Source
	tgt Target
	mir Mirror
	dry bool

	queue

	state stateStore

	hooksMu   sync.Mutex
	hooksMode string

	wake chan struct{}

	delMu     sync.Mutex
	deletions []time.Time
}

func New(cfg *config.Config, src Source, tgt Target, mir Mirror) (*Engine, error) {
	e := &Engine{
		cfg: cfg, src: src, tgt: tgt, mir: mir, dry: cfg.Sync.DryRun,
		wake: make(chan struct{}, 1),
	}
	e.queue.init()
	if err := e.state.load(cfg.Paths.StateDir, e.dry); err != nil {
		return nil, err
	}
	return e, nil
}

func (e *Engine) Config() *config.Config {
	e.cfgMu.RLock()
	defer e.cfgMu.RUnlock()
	return e.cfg
}

func (e *Engine) Dry() bool { return e.dry }

func (e *Engine) Wake() {
	select {
	case e.wake <- struct{}{}:
	default:
	}
}

func (e *Engine) Reload(c *config.Config) {
	c.Paths.StateDir = e.Config().Paths.StateDir
	e.cfgMu.Lock()
	e.cfg = c
	e.cfgMu.Unlock()
	e.state.clearRetries()
	e.Wake()
}
