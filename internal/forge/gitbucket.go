package forge

import (
	"encoding/base64"
	"encoding/json"
	"errors"
	"fmt"
	"net/http"
	"net/url"
	"strings"
	"sync"

	"github.com/cristobaltormo/git-sync/internal/config"
	"github.com/cristobaltormo/git-sync/internal/httpx"
	"github.com/cristobaltormo/git-sync/internal/logx"
)

var errNoGlobalHooks = errors.New("this server has no system-wide webhooks")

type GitBucket struct {
	cfg   *config.Config
	base  string
	http  *httpx.Client
	pages pageCache
	mu    sync.Mutex
	login string
}

type gbRepo struct {
	ID            int64   `json:"id"`
	Name          string  `json:"name"`
	FullName      string  `json:"full_name"`
	Private       bool    `json:"private"`
	Description   *string `json:"description"`
	Homepage      *string `json:"homepage"`
	DefaultBranch string  `json:"default_branch"`
	Fork          bool    `json:"fork"`
	Archived      bool    `json:"archived"`
	UpdatedAt     *string `json:"updated_at"`
	PushedAt      *string `json:"pushed_at"`
	Owner         struct {
		Login string `json:"login"`
	} `json:"owner"`
}

func (g *gbRepo) repo() *Repo {
	updated := ""
	switch {
	case g.PushedAt != nil:
		updated = *g.PushedAt
	case g.UpdatedAt != nil:
		updated = *g.UpdatedAt
	}
	owner := g.Owner.Login
	if owner == "" {
		owner, _, _ = strings.Cut(g.FullName, "/")
	}
	return &Repo{
		ID: g.ID, Owner: owner, Name: g.Name, Private: g.Private, Description: strPtr(g.Description),
		Website: strPtr(g.Homepage), Archived: g.Archived, DefaultBranch: g.DefaultBranch,
		Fork: g.Fork, Updated: updated,
	}
}

func strPtr(s *string) string {
	if s == nil {
		return ""
	}
	return strings.TrimSpace(*s)
}

func newGitBucket(cfg *config.Config) *GitBucket {
	return &GitBucket{
		cfg: cfg, base: cfg.SourceURL() + "/api/v3",
		http: httpx.New(map[string]string{
			"Authorization": "token " + cfg.Source.Token,
			"Accept":        "application/json", "User-Agent": UserAgent,
		}),
	}
}

func (g *GitBucket) Kind() string { return "gitbucket" }

func (g *GitBucket) get(path string) (*httpx.Response, error) {
	return g.http.Do("GET", g.base+path, nil)
}

func (g *GitBucket) Whoami() (*Identity, error) {
	r, err := g.get("/user")
	if err != nil {
		return nil, err
	}
	if r.Status == 401 {
		return nil, &httpx.APIError{Status: 401, Msg: "the source token was rejected"}
	}
	if !r.OK() {
		return nil, httpx.Fail(r, "")
	}
	var u struct {
		Login string `json:"login"`
	}
	if err := r.Decode(&u); err != nil {
		return nil, err
	}
	g.mu.Lock()
	g.login = u.Login
	g.mu.Unlock()
	return &Identity{Login: u.Login}, nil
}

func (g *GitBucket) OwnerExists(owner string) (bool, error) {
	r, err := g.get("/users/" + url.PathEscape(owner))
	if err != nil {
		return false, err
	}
	return r.OK(), nil
}

func (g *GitBucket) List(owner string) ([]*Repo, error) {
	r, err := g.get("/users/" + url.PathEscape(owner) + "/repos")
	if err != nil {
		return nil, err
	}
	if !r.OK() {
		return nil, httpx.Fail(r, "listing "+owner)
	}
	page, err := g.pages.decode("list/"+owner, r.Body, func(b []byte) ([]*Repo, error) {
		var rows []*gbRepo
		if err := json.Unmarshal(b, &rows); err != nil {
			return nil, err
		}
		repos := make([]*Repo, len(rows))
		for i, x := range rows {
			repos[i] = x.repo()
		}
		return repos, nil
	})
	if err != nil {
		return nil, err
	}
	var out []*Repo
	for _, x := range page {
		if strings.EqualFold(x.Owner, owner) {
			out = append(out, x)
		}
	}
	return out, nil
}

func (g *GitBucket) Get(owner, name string) (*Repo, error) {
	r, err := g.get("/repos/" + url.PathEscape(owner) + "/" + url.PathEscape(name))
	if err != nil {
		return nil, err
	}
	if r.Status == 404 {
		return nil, nil
	}
	if !r.OK() {
		return nil, httpx.Fail(r, "")
	}
	var x gbRepo
	if err := r.Decode(&x); err != nil {
		return nil, err
	}
	return x.repo(), nil
}

func (g *GitBucket) CloneURL(r *Repo) string {
	return fmt.Sprintf("%s/git/%s/%s.git", g.cfg.SourceURL(), r.Owner, r.Name)
}

func (g *GitBucket) GitHeader() (string, error) {
	g.mu.Lock()
	login := g.login
	g.mu.Unlock()
	if login == "" {
		id, err := g.Whoami()
		if err != nil {
			return "", err
		}
		login = id.Login
	}
	b := base64.StdEncoding.EncodeToString([]byte(login + ":" + g.cfg.Source.Token))
	logx.Hide(b)
	return "Authorization: Basic " + b, nil
}

func (g *GitBucket) ParseWebhook(h http.Header, body []byte, secret string) (*Event, error) {
	if !validMAC(secret, body, h.Get("X-Hub-Signature-256"), "sha256=") {
		return nil, ErrSignature
	}
	switch h.Get("X-Github-Event") {
	case "push", "create", "delete", "repository":
	default:
		return nil, nil
	}
	var p struct {
		Repository struct {
			ID       int64  `json:"id"`
			FullName string `json:"full_name"`
		} `json:"repository"`
	}
	if err := json.Unmarshal(body, &p); err != nil {
		return nil, err
	}
	owner, name, ok := strings.Cut(p.Repository.FullName, "/")
	if !ok {
		return nil, nil
	}
	return &Event{Kind: Changed, Owner: owner, Name: name, ID: p.Repository.ID}, nil
}

func (g *GitBucket) Hooks() Hooks { return &gbHooks{g} }

type gbHooks struct{ g *GitBucket }

func (h *gbHooks) path(r *Repo) string {
	return "/repos/" + url.PathEscape(r.Owner) + "/" + url.PathEscape(r.Name) + "/hooks"
}

func (h *gbHooks) body() map[string]any {
	return map[string]any{
		"name": "web", "active": true, "events": []string{"push", "create", "delete"},
		"config": map[string]string{"url": h.g.cfg.HookURL(), "content_type": "json", "secret": h.g.cfg.Listen.Secret},
	}
}

func (h *gbHooks) ours(path string) ([]hookInfo, error) {
	r, err := h.g.get(path)
	if err != nil {
		return nil, err
	}
	if !r.OK() {
		return nil, httpx.Fail(r, path)
	}
	var all []hookInfo
	if err := r.Decode(&all); err != nil {
		return nil, err
	}
	var mine []hookInfo
	for _, x := range all {
		if x.Config.URL == h.g.cfg.HookURL() {
			mine = append(mine, x)
		}
	}
	return mine, nil
}

func (h *gbHooks) Repo(r *Repo, force bool) (string, error) {
	mine, err := h.ours(h.path(r))
	if err != nil {
		return "", err
	}
	if len(mine) > 0 && !force {
		return "exists", nil
	}
	method, target, result := "POST", h.g.base+h.path(r), "created"
	if len(mine) > 0 {
		method, target, result = "PATCH", fmt.Sprintf("%s%s/%d", h.g.base, h.path(r), mine[0].ID), "updated"
	}
	resp, err := h.g.http.Do(method, target, h.body())
	if err != nil {
		return "", err
	}
	if !resp.OK() {
		return "", httpx.Fail(resp, "")
	}
	return result, nil
}

func (h *gbHooks) RemoveRepo(r *Repo) int {
	mine, err := h.ours(h.path(r))
	if err != nil {
		return 0
	}
	n := 0
	for _, x := range mine {
		if d, err := h.g.http.Do("DELETE", fmt.Sprintf("%s%s/%d", h.g.base, h.path(r), x.ID), nil); err == nil && d.OK() {
			n++
		}
	}
	return n
}

func (h *gbHooks) Global(int64, bool) (int64, string, error) { return 0, "", errNoGlobalHooks }
func (h *gbHooks) RemoveGlobal(int64) bool                   { return false }
