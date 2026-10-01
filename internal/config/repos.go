package config

import (
	"bytes"
	"os"
	"path/filepath"
	"regexp"
	"sort"
	"strings"

	"github.com/BurntSushi/toml"
)

type RepoRule struct {
	Sync        *bool  `toml:"sync,omitempty"`
	Name        string `toml:"name,omitempty"`
	KeepPrivate *bool  `toml:"keep_private,omitempty"`
	Tags        *bool  `toml:"tags,omitempty"`
	Metadata    *bool  `toml:"metadata,omitempty"`
	Prune       *bool  `toml:"prune,omitempty"`
	OnDelete    string `toml:"on_delete,omitempty"`
	Note        string `toml:"note,omitempty"`
}

func (r *RepoRule) empty() bool {
	return r.Sync == nil && r.Name == "" && r.KeepPrivate == nil && r.Tags == nil &&
		r.Metadata == nil && r.Prune == nil && r.OnDelete == "" && r.Note == ""
}

type Repos struct {
	Path  string               `toml:"-"`
	Rules map[string]*RepoRule `toml:"repos"`
}

var ghName = regexp.MustCompile(`^[A-Za-z0-9._-]{1,100}$`)

func ReposPath(cfg *Config) string {
	if cfg.Paths.ReposFile != "" {
		return cfg.Paths.ReposFile
	}
	if cfg.Path != "" {
		return filepath.Join(filepath.Dir(cfg.Path), "repos.toml")
	}
	return filepath.Join(cfg.Paths.StateDir, "repos.toml")
}

func LoadRepos(path string) (*Repos, error) {
	r := &Repos{Path: path, Rules: map[string]*RepoRule{}}
	md, err := toml.DecodeFile(path, r)
	if err != nil {
		if os.IsNotExist(err) {
			return r, nil
		}
		return nil, Errorf("%s: %v", path, err)
	}
	if un := md.Undecoded(); len(un) > 0 {
		names := make([]string, len(un))
		for i, k := range un {
			names[i] = k.String()
		}
		sort.Strings(names)
		return nil, Errorf("unknown option(s) in %s: %s", path, strings.Join(names, ", "))
	}
	if r.Rules == nil {
		r.Rules = map[string]*RepoRule{}
	}
	norm := make(map[string]*RepoRule, len(r.Rules))
	for k, rule := range r.Rules {
		if rule == nil {
			continue
		}
		if err := rule.validate(k); err != nil {
			return nil, Errorf("%s: %v", path, err)
		}
		norm[strings.ToLower(k)] = rule
	}
	r.Rules = norm
	return r, nil
}

func (r *RepoRule) validate(key string) error {
	if r.Name != "" && !ghName.MatchString(r.Name) {
		return Errorf("repos.%q: name %q is not a valid GitHub repository name", key, r.Name)
	}
	if r.OnDelete != "" && !oneOf(r.OnDelete, "delete", "archive", "ignore") {
		return Errorf("repos.%q: on_delete must be delete, archive or ignore", key)
	}
	return nil
}

func (r *Repos) Rule(full string) *RepoRule {
	if r == nil {
		return nil
	}
	return r.Rules[strings.ToLower(full)]
}

func (r *Repos) Edit(full string, fn func(*RepoRule)) error {
	key := strings.ToLower(full)
	rule := r.Rules[key]
	if rule == nil {
		rule = &RepoRule{}
	}
	cp := *rule
	fn(&cp)
	if err := cp.validate(full); err != nil {
		return err
	}
	if cp.empty() {
		delete(r.Rules, key)
	} else {
		r.Rules[key] = &cp
	}
	return nil
}

const reposHeader = `# Per-repository decisions and overrides, written by "gitsync repos".
# Safe to edit by hand; a running gitsync picks changes up on its own.
#
#   sync         true: always sync it, false: never sync it (beats every filter)
#   name         repository name to use on GitHub
#   keep_private never make the GitHub copy public
#   tags         mirror tags (overrides sync.tags)
#   metadata     copy description and topics (overrides sync.metadata)
#   prune        false: keep branches that exist only on GitHub (pull request heads)
#   on_delete    delete, archive or ignore when it disappears from the source
#   note         free text for you

`

func (r *Repos) Save() error {
	var body bytes.Buffer
	enc := toml.NewEncoder(&body)
	enc.Indent = ""
	if err := enc.Encode(struct {
		Rules map[string]*RepoRule `toml:"repos"`
	}{r.Rules}); err != nil {
		return err
	}
	var buf bytes.Buffer
	buf.WriteString(reposHeader)
	buf.WriteString(strings.TrimPrefix(strings.TrimLeft(body.String(), "\n"), "[repos]\n"))
	if err := os.MkdirAll(filepath.Dir(r.Path), 0o755); err != nil {
		return Errorf("cannot create %s: %v", filepath.Dir(r.Path), err)
	}
	tmp, err := os.CreateTemp(filepath.Dir(r.Path), ".repos-*.toml")
	if err != nil {
		return Errorf("cannot write %s: %v", r.Path, err)
	}
	defer os.Remove(tmp.Name())
	if _, err := tmp.Write(buf.Bytes()); err != nil {
		tmp.Close()
		return err
	}
	mode := os.FileMode(0o644)
	prev, prevErr := os.Stat(r.Path)
	if prevErr == nil {
		mode = prev.Mode().Perm()
	}
	if err := tmp.Chmod(mode); err != nil && !isWindows() {
		tmp.Close()
		return err
	}
	if err := tmp.Close(); err != nil {
		return err
	}
	if prevErr == nil {
		keepOwner(tmp.Name(), prev)
	}
	return os.Rename(tmp.Name(), r.Path)
}
