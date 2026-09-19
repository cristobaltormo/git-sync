package forge

import (
	"crypto/hmac"
	"crypto/sha1"
	"crypto/sha256"
	"encoding/hex"
	"errors"
	"fmt"
	"net/http"
	"sort"
	"strings"
	"sync"

	"github.com/cristobaltormo/git-sync/internal/config"
)

type Repo struct {
	ID            int64
	Owner         string
	Name          string
	Private       bool
	Description   string
	Website       string
	Archived      bool
	DefaultBranch string
	Fork          bool
	Mirror        bool
	Empty         bool
	Topics        []string
	Updated       string
}

func (r *Repo) Full() string {
	if r.Owner == OneDevRoot {
		return r.Name
	}
	return r.Owner + "/" + r.Name
}

func (r *Repo) Sig() string {
	topics := append([]string(nil), r.Topics...)
	sort.Strings(topics)
	s := fmt.Sprintf("%s|%s|%t|%s|%s|%t|%s|%s|%t", r.Owner, r.Name, r.Private, r.Description,
		r.Website, r.Archived, r.DefaultBranch, strings.Join(topics, ","), r.Fork)
	sum := sha1.Sum([]byte(s))
	return hex.EncodeToString(sum[:8])
}

type Identity struct {
	Login       string
	Admin       bool
	SystemHooks bool
}

type Change int

const (
	Changed Change = iota + 1
	Removed
)

type Event struct {
	Kind  Change
	Owner string
	Name  string
	ID    int64
}

var ErrSignature = errors.New("bad signature")

type Hooks interface {
	Global(known int64, force bool) (id int64, result string, err error)
	Repo(r *Repo, force bool) (string, error)
	RemoveGlobal(id int64) bool
	RemoveRepo(r *Repo) int
}

type Provider interface {
	Kind() string
	Whoami() (*Identity, error)
	List(owner string) ([]*Repo, error)
	Get(owner, name string) (*Repo, error)
	OwnerExists(owner string) (bool, error)
	CloneURL(r *Repo) string
	GitHeader() (string, error)
	Hooks() Hooks
	ParseWebhook(h http.Header, body []byte, secret string) (*Event, error)
}

func New(cfg *config.Config) (Provider, error) {
	switch cfg.Source.Type {
	case "forgejo", "gitea", "codeberg", "gogs":
		return newGitea(cfg), nil
	case "gitlab":
		return newGitLab(cfg), nil
	case "gitbucket":
		return newGitBucket(cfg), nil
	case "onedev":
		return newOneDev(cfg), nil
	}
	return nil, fmt.Errorf("unsupported source type %q", cfg.Source.Type)
}

func validMAC(secret string, body []byte, got, prefix string) bool {
	got = strings.TrimSpace(got)
	if prefix != "" {
		if !strings.HasPrefix(got, prefix) {
			return false
		}
		got = got[len(prefix):]
	}
	mac := hmac.New(sha256.New, []byte(secret))
	mac.Write(body)
	return hmac.Equal([]byte(got), []byte(hex.EncodeToString(mac.Sum(nil))))
}

type pageCache struct {
	mu    sync.Mutex
	pages map[string]*cachedPage
}

type cachedPage struct {
	sum   [sha1.Size]byte
	repos []*Repo
}

// skips decoding a page whose bytes are identical to the last poll
func (c *pageCache) decode(key string, body []byte, fn func([]byte) ([]*Repo, error)) ([]*Repo, error) {
	sum := sha1.Sum(body)
	c.mu.Lock()
	cached := c.pages[key]
	c.mu.Unlock()
	if cached != nil && cached.sum == sum {
		return cached.repos, nil
	}
	repos, err := fn(body)
	if err != nil {
		return nil, err
	}
	c.mu.Lock()
	if c.pages == nil {
		c.pages = map[string]*cachedPage{}
	}
	c.pages[key] = &cachedPage{sum: sum, repos: repos}
	c.mu.Unlock()
	return repos, nil
}

const UserAgent = "gitsync"
