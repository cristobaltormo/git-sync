package engine

import (
	"encoding/json"
	"os"
	"path/filepath"
	"strconv"
	"sync"

	"github.com/cristobaltormo/git-sync/internal/config"
	"github.com/cristobaltormo/git-sync/internal/github"
	"github.com/cristobaltormo/git-sync/internal/logx"
)

type Entry struct {
	Owner         string       `json:"owner"`
	Name          string       `json:"name"`
	GHOwner       string       `json:"gh_owner,omitempty"`
	GHName        string       `json:"gh_name,omitempty"`
	Managed       bool         `json:"managed"`
	Sig           string       `json:"sig,omitempty"`
	Updated       string       `json:"updated,omitempty"`
	GH            *github.Snap `json:"gh,omitempty"`
	LastOK        int64        `json:"last_ok,omitempty"`
	LastPush      int64        `json:"last_push,omitempty"`
	Refs          string       `json:"refs,omitempty"`
	LastError     string       `json:"last_error,omitempty"`
	Attempts      int          `json:"attempts,omitempty"`
	RetryAt       int64        `json:"retry_at,omitempty"`
	MissingSince  int64        `json:"missing_since,omitempty"`
	Excluded      bool         `json:"excluded,omitempty"`
	Hook          string       `json:"hook,omitempty"`
	RefusedBranch string       `json:"refused_branch,omitempty"`
}

type State struct {
	Version    int               `json:"version"`
	SystemHook int64             `json:"system_hook,omitempty"`
	Repos      map[string]*Entry `json:"repos"`
}

type stateStore struct {
	mu   sync.Mutex
	data State
	file string
	dry  bool
}

func ReadState(dir string) (*State, error) {
	b, err := os.ReadFile(filepath.Join(dir, "state.json"))
	if err != nil {
		return nil, err
	}
	var st State
	if err := json.Unmarshal(b, &st); err != nil {
		return nil, err
	}
	return &st, nil
}

func (s *stateStore) load(dir string, dry bool) error {
	s.file, s.dry = filepath.Join(dir, "state.json"), dry
	s.data = State{Version: 1, Repos: map[string]*Entry{}}
	st, err := ReadState(dir)
	if os.IsNotExist(err) {
		return nil
	}
	if err != nil {
		return config.Errorf("cannot read %s: %v", s.file, err)
	}
	if st.Repos == nil {
		st.Repos = map[string]*Entry{}
	}
	s.data = *st
	return nil
}

func (s *stateStore) save() {
	if s.dry {
		return
	}
	s.mu.Lock()
	blob, err := json.MarshalIndent(s.data, "", " ")
	s.mu.Unlock()
	if err != nil {
		logx.Errorf("cannot encode state: %v", err)
		return
	}
	if err := os.MkdirAll(filepath.Dir(s.file), 0o755); err != nil {
		logx.Errorf("cannot create state dir: %v", err)
		return
	}
	tmp := s.file + ".tmp"
	if err := os.WriteFile(tmp, blob, 0o600); err != nil {
		logx.Errorf("cannot write state: %v", err)
		return
	}
	if err := os.Rename(tmp, s.file); err != nil {
		logx.Errorf("cannot write state: %v", err)
	}
}

func key(id int64) string { return strconv.FormatInt(id, 10) }

func (s *stateStore) get(id int64) (Entry, bool) {
	s.mu.Lock()
	defer s.mu.Unlock()
	x, ok := s.data.Repos[key(id)]
	if !ok {
		return Entry{}, false
	}
	c := *x
	if x.GH != nil {
		g := x.GH.Clone()
		c.GH = &g
	}
	return c, true
}

func (s *stateStore) update(id int64, create bool, fn func(*Entry)) {
	s.mu.Lock()
	defer s.mu.Unlock()
	x, ok := s.data.Repos[key(id)]
	if !ok {
		if !create {
			return
		}
		x = &Entry{}
		s.data.Repos[key(id)] = x
	}
	fn(x)
}

func (s *stateStore) drop(id int64) {
	s.mu.Lock()
	delete(s.data.Repos, key(id))
	s.mu.Unlock()
}

func (s *stateStore) all() map[int64]Entry {
	s.mu.Lock()
	defer s.mu.Unlock()
	out := make(map[int64]Entry, len(s.data.Repos))
	for k, v := range s.data.Repos {
		id, _ := strconv.ParseInt(k, 10, 64)
		out[id] = *v
	}
	return out
}

func (s *stateStore) clearRetries() {
	s.mu.Lock()
	defer s.mu.Unlock()
	for _, x := range s.data.Repos {
		if x.LastError != "" {
			x.RetryAt = 0
		}
	}
}

func (s *stateStore) systemHook() int64 {
	s.mu.Lock()
	defer s.mu.Unlock()
	return s.data.SystemHook
}

func (s *stateStore) setSystemHook(id int64) {
	s.mu.Lock()
	s.data.SystemHook = id
	s.mu.Unlock()
	s.save()
}

func (e *Engine) Entries() map[int64]Entry { return e.state.all() }

func (e *Engine) SystemHook() int64 { return e.state.systemHook() }

func (e *Engine) SetSystemHook(id int64) { e.state.setSystemHook(id) }
