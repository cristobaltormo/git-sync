package forge

import (
	"crypto/subtle"
	"encoding/base64"
	"encoding/json"
	"fmt"
	"net/http"
	"net/url"
	"strconv"
	"strings"
	"sync"

	"github.com/cristobaltormo/git-sync/internal/config"
	"github.com/cristobaltormo/git-sync/internal/httpx"
	"github.com/cristobaltormo/git-sync/internal/logx"
)

type GitLab struct {
	cfg   *config.Config
	base  string
	http  *httpx.Client
	pages pageCache
	mu    sync.Mutex
	ns    map[string]glNamespace
}

type glNamespace struct {
	ID   int64
	Kind string
}

type glProject struct {
	ID                int64    `json:"id"`
	Path              string   `json:"path"`
	PathWithNamespace string   `json:"path_with_namespace"`
	Description       string   `json:"description"`
	Visibility        string   `json:"visibility"`
	Archived          bool     `json:"archived"`
	DefaultBranch     string   `json:"default_branch"`
	EmptyRepo         bool     `json:"empty_repo"`
	Mirror            bool     `json:"mirror"`
	Topics            []string `json:"topics"`
	LastActivityAt    string   `json:"last_activity_at"`
	MarkedForDeletion string   `json:"marked_for_deletion_at"`
	ForkedFrom        *struct {
		ID int64 `json:"id"`
	} `json:"forked_from_project"`
	Namespace struct {
		FullPath string `json:"full_path"`
	} `json:"namespace"`
}

func (p *glProject) repo() *Repo {
	owner := p.Namespace.FullPath
	if owner == "" {
		owner, _, _ = strings.Cut(p.PathWithNamespace, "/")
	}
	return &Repo{
		ID: p.ID, Owner: owner, Name: p.Path, Private: p.Visibility != "public",
		Description: strings.TrimSpace(p.Description), Archived: p.Archived,
		DefaultBranch: p.DefaultBranch, Fork: p.ForkedFrom != nil, Mirror: p.Mirror,
		Empty: p.EmptyRepo, Topics: p.Topics, Updated: p.LastActivityAt,
	}
}

func newGitLab(cfg *config.Config) *GitLab {
	return &GitLab{
		cfg: cfg, base: cfg.SourceURL() + "/api/v4",
		http: httpx.New(map[string]string{
			"Authorization": "Bearer " + cfg.Source.Token,
			"Accept":        "application/json", "User-Agent": UserAgent,
		}),
		ns: map[string]glNamespace{},
	}
}

func (g *GitLab) Kind() string { return "gitlab" }

func (g *GitLab) get(path string, q url.Values) (*httpx.Response, error) {
	u := g.base + path
	if len(q) > 0 {
		u += "?" + q.Encode()
	}
	return g.http.Do("GET", u, nil)
}

func (g *GitLab) Whoami() (*Identity, error) {
	r, err := g.get("/user", nil)
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
		Username string `json:"username"`
		IsAdmin  bool   `json:"is_admin"`
	}
	if err := r.Decode(&u); err != nil {
		return nil, err
	}
	return &Identity{Login: u.Username, Admin: u.IsAdmin, SystemHooks: u.IsAdmin}, nil
}

func (g *GitLab) namespace(path string) (glNamespace, error) {
	g.mu.Lock()
	n, ok := g.ns[path]
	g.mu.Unlock()
	if ok {
		return n, nil
	}
	r, err := g.get("/namespaces/"+url.PathEscape(path), nil)
	if err != nil {
		return n, err
	}
	if !r.OK() {
		return n, &httpx.APIError{Status: r.Status, Msg: "cannot look up namespace '" + path + "'"}
	}
	var v struct {
		ID   int64  `json:"id"`
		Kind string `json:"kind"`
	}
	if err := r.Decode(&v); err != nil {
		return n, err
	}
	n = glNamespace{ID: v.ID, Kind: v.Kind}
	g.mu.Lock()
	g.ns[path] = n
	g.mu.Unlock()
	return n, nil
}

func (g *GitLab) OwnerExists(owner string) (bool, error) {
	r, err := g.get("/namespaces/"+url.PathEscape(owner), nil)
	if err != nil {
		return false, err
	}
	return r.OK(), nil
}

func (g *GitLab) List(owner string) ([]*Repo, error) {
	ns, err := g.namespace(owner)
	if err != nil {
		return nil, err
	}
	base := fmt.Sprintf("/groups/%d/projects", ns.ID)
	if ns.Kind == "user" {
		base = fmt.Sprintf("/users/%d/projects", ns.ID)
	}
	var out []*Repo
	for n := 1; ; n++ {
		q := url.Values{"per_page": {"100"}, "page": {strconv.Itoa(n)}}
		if ns.Kind != "user" {
			q.Set("include_subgroups", "false")
		}
		r, err := g.get(base, q)
		if err != nil {
			return nil, err
		}
		if !r.OK() {
			return nil, httpx.Fail(r, "listing "+owner)
		}
		page, err := g.pages.decode(base+"?"+q.Encode(), r.Body, func(b []byte) ([]*Repo, error) {
			var rows []*glProject
			if err := json.Unmarshal(b, &rows); err != nil {
				return nil, err
			}
			var repos []*Repo
			for _, p := range rows {
				// deleted projects stay listed for a while, renamed and marked
				if p.MarkedForDeletion == "" {
					repos = append(repos, p.repo())
				}
			}
			return repos, nil
		})
		if err != nil {
			return nil, err
		}
		out = append(out, page...)
		if r.Header.Get("X-Next-Page") == "" {
			return out, nil
		}
	}
}

func (g *GitLab) Get(owner, name string) (*Repo, error) {
	r, err := g.get("/projects/"+url.PathEscape(owner+"/"+name), nil)
	if err != nil {
		return nil, err
	}
	if r.Status == 404 {
		return nil, nil
	}
	if !r.OK() {
		return nil, httpx.Fail(r, "")
	}
	var p glProject
	if err := r.Decode(&p); err != nil {
		return nil, err
	}
	if p.MarkedForDeletion != "" {
		return nil, nil
	}
	return p.repo(), nil
}

func (g *GitLab) CloneURL(r *Repo) string {
	return fmt.Sprintf("%s/%s/%s.git", g.cfg.SourceURL(), r.Owner, r.Name)
}

func (g *GitLab) GitHeader() (string, error) {
	b := base64.StdEncoding.EncodeToString([]byte("oauth2:" + g.cfg.Source.Token))
	logx.Hide(b)
	return "Authorization: Basic " + b, nil
}

func (g *GitLab) ParseWebhook(h http.Header, body []byte, secret string) (*Event, error) {
	if subtle.ConstantTimeCompare([]byte(h.Get("X-Gitlab-Token")), []byte(secret)) != 1 {
		return nil, ErrSignature
	}
	var p struct {
		EventName         string `json:"event_name"`
		ProjectID         int64  `json:"project_id"`
		PathWithNamespace string `json:"path_with_namespace"`
		Project           struct {
			ID                int64  `json:"id"`
			PathWithNamespace string `json:"path_with_namespace"`
		} `json:"project"`
	}
	if err := json.Unmarshal(body, &p); err != nil {
		return nil, err
	}
	path, id := p.PathWithNamespace, p.ProjectID
	if p.Project.PathWithNamespace != "" {
		path, id = p.Project.PathWithNamespace, p.Project.ID
	}
	i := strings.LastIndex(path, "/")
	if i < 1 {
		return nil, nil
	}
	ev := &Event{Kind: Changed, Owner: path[:i], Name: path[i+1:], ID: id}
	if p.EventName == "project_destroy" {
		ev.Kind = Removed
	}
	return ev, nil
}

func (g *GitLab) Hooks() Hooks { return &glHooks{g} }

type glHooks struct{ g *GitLab }

type glHook struct {
	ID  int64  `json:"id"`
	URL string `json:"url"`
}

func (h *glHooks) body() map[string]any {
	return map[string]any{
		"url": h.g.cfg.HookURL(), "token": h.g.cfg.Listen.Secret,
		"push_events": true, "tag_push_events": true, "repository_update_events": true,
		"enable_ssl_verification": true,
	}
}

func (h *glHooks) ours(path string) ([]glHook, error) {
	r, err := h.g.get(path, url.Values{"per_page": {"100"}})
	if err != nil {
		return nil, err
	}
	if !r.OK() {
		return nil, httpx.Fail(r, path)
	}
	var all []glHook
	if err := r.Decode(&all); err != nil {
		return nil, err
	}
	var mine []glHook
	for _, x := range all {
		if x.URL == h.g.cfg.HookURL() {
			mine = append(mine, x)
		}
	}
	return mine, nil
}

func (h *glHooks) ensure(path string, force bool) (int64, string, error) {
	mine, err := h.ours(path)
	if err != nil {
		return 0, "", err
	}
	if len(mine) > 0 && !force {
		return mine[0].ID, "exists", nil
	}
	method, target, result := "POST", h.g.base+path, "created"
	if len(mine) > 0 {
		method, target, result = "PUT", fmt.Sprintf("%s%s/%d", h.g.base, path, mine[0].ID), "updated"
	}
	r, err := h.g.http.Do(method, target, h.body())
	if err != nil {
		return 0, "", err
	}
	if !r.OK() {
		return 0, "", httpx.Fail(r, "")
	}
	var created glHook
	r.Decode(&created)
	return created.ID, result, nil
}

func (h *glHooks) Global(known int64, force bool) (int64, string, error) {
	return h.ensure("/hooks", force)
}

func (h *glHooks) Repo(r *Repo, force bool) (string, error) {
	_, res, err := h.ensure(fmt.Sprintf("/projects/%s/hooks", url.PathEscape(r.Owner+"/"+r.Name)), force)
	return res, err
}

func (h *glHooks) RemoveGlobal(id int64) bool {
	mine, err := h.ours("/hooks")
	if err != nil {
		return false
	}
	n := 0
	for _, x := range mine {
		if d, err := h.g.http.Do("DELETE", fmt.Sprintf("%s/hooks/%d", h.g.base, x.ID), nil); err == nil && d.OK() {
			n++
		}
	}
	return n > 0
}

func (h *glHooks) RemoveRepo(r *Repo) int {
	path := fmt.Sprintf("/projects/%s/hooks", url.PathEscape(r.Owner+"/"+r.Name))
	mine, err := h.ours(path)
	if err != nil {
		return 0
	}
	n := 0
	for _, x := range mine {
		if d, err := h.g.http.Do("DELETE", fmt.Sprintf("%s%s/%d", h.g.base, path, x.ID), nil); err == nil && d.OK() {
			n++
		}
	}
	return n
}
