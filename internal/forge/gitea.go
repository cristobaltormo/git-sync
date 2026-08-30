package forge

import (
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

var (
	giteaHookEvents = []string{"push", "create", "delete", "repository"}
	gogsHookEvents  = []string{"push", "create", "delete"}
)

type Gitea struct {
	cfg   *config.Config
	kind  string
	base  string
	http  *httpx.Client
	pages pageCache
	mu    sync.Mutex
	ids   map[string]int64
	login string
}

type giteaRepo struct {
	ID       int64  `json:"id"`
	Name     string `json:"name"`
	FullName string `json:"full_name"`
	Owner    struct {
		Login    string `json:"login"`
		Username string `json:"username"`
	} `json:"owner"`
	Private       bool     `json:"private"`
	Description   string   `json:"description"`
	Website       string   `json:"website"`
	Archived      bool     `json:"archived"`
	DefaultBranch string   `json:"default_branch"`
	Fork          bool     `json:"fork"`
	Mirror        bool     `json:"mirror"`
	Empty         bool     `json:"empty"`
	Topics        []string `json:"topics"`
	UpdatedAt     string   `json:"updated_at"`
}

func (g *giteaRepo) repo() *Repo {
	owner := g.Owner.Login
	if owner == "" {
		owner = g.Owner.Username
	}
	if owner == "" {
		owner, _, _ = strings.Cut(g.FullName, "/")
	}
	name := g.Name
	if name == "" {
		name = g.FullName[strings.LastIndex(g.FullName, "/")+1:]
	}
	return &Repo{
		ID: g.ID, Owner: owner, Name: name, Private: g.Private,
		Description: strings.TrimSpace(g.Description), Website: strings.TrimSpace(g.Website),
		Archived: g.Archived, DefaultBranch: g.DefaultBranch, Fork: g.Fork, Mirror: g.Mirror,
		Empty: g.Empty, Topics: g.Topics, Updated: g.UpdatedAt,
	}
}

func newGitea(cfg *config.Config) *Gitea {
	kind := cfg.Source.Type
	if kind == "codeberg" {
		kind = "forgejo"
	}
	return &Gitea{
		cfg: cfg, kind: kind, base: cfg.SourceURL() + "/api/v1",
		http: httpx.New(map[string]string{
			"Authorization": "token " + cfg.Source.Token,
			"Accept":        "application/json", "User-Agent": UserAgent,
		}),
		ids: map[string]int64{},
	}
}

func (g *Gitea) Kind() string { return g.kind }

func (g *Gitea) get(path string, q url.Values) (*httpx.Response, error) {
	u := g.base + path
	if len(q) > 0 {
		u += "?" + q.Encode()
	}
	return g.http.Do("GET", u, nil)
}

func (g *Gitea) Whoami() (*Identity, error) {
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
		Login    string `json:"login"`
		Username string `json:"username"`
		IsAdmin  bool   `json:"is_admin"`
	}
	if err := r.Decode(&u); err != nil {
		return nil, err
	}
	// Gitea only applies a system webhook created through the API after a restart
	id := &Identity{Login: u.Login, Admin: u.IsAdmin, SystemHooks: u.IsAdmin && g.kind != "gitea"}
	if id.Login == "" {
		id.Login = u.Username
	}
	g.mu.Lock()
	g.login = id.Login
	g.mu.Unlock()
	return id, nil
}

func (g *Gitea) ownerID(owner string) (int64, error) {
	g.mu.Lock()
	id, ok := g.ids[owner]
	g.mu.Unlock()
	if ok {
		return id, nil
	}
	r, err := g.get("/users/"+url.PathEscape(owner), nil)
	if err != nil {
		return 0, err
	}
	if !r.OK() {
		return 0, &httpx.APIError{Status: r.Status, Msg: "cannot look up source owner '" + owner + "'"}
	}
	var u struct {
		ID int64 `json:"id"`
	}
	if err := r.Decode(&u); err != nil {
		return 0, err
	}
	g.mu.Lock()
	g.ids[owner] = u.ID
	g.mu.Unlock()
	return u.ID, nil
}

func (g *Gitea) OwnerExists(owner string) (bool, error) {
	r, err := g.get("/users/"+url.PathEscape(owner), nil)
	if err != nil {
		return false, err
	}
	return r.OK(), nil
}

func (g *Gitea) List(owner string) ([]*Repo, error) {
	if g.kind == "gogs" {
		return g.listPlain(owner)
	}
	return g.listSearch(owner)
}

func (g *Gitea) listSearch(owner string) ([]*Repo, error) {
	uid, err := g.ownerID(owner)
	if err != nil {
		return nil, err
	}
	var out []*Repo
	for n := 1; ; n++ {
		q := url.Values{"uid": {strconv.FormatInt(uid, 10)}, "page": {strconv.Itoa(n)}, "limit": {"50"}}
		r, err := g.get("/repos/search", q)
		if err != nil {
			return nil, err
		}
		if !r.OK() {
			return nil, httpx.Fail(r, "listing "+owner)
		}
		page, err := g.pages.decode("search?"+q.Encode(), r.Body, decodeGiteaSearch)
		if err != nil {
			return nil, err
		}
		for _, x := range page {
			// the search index lags after a transfer
			if strings.EqualFold(x.Owner, owner) {
				out = append(out, x)
			}
		}
		if len(page) < 50 {
			return out, nil
		}
	}
}

func decodeGiteaSearch(b []byte) ([]*Repo, error) {
	var body struct {
		Data []*giteaRepo `json:"data"`
	}
	if err := json.Unmarshal(b, &body); err != nil {
		return nil, err
	}
	return convertGitea(body.Data), nil
}

func convertGitea(rows []*giteaRepo) []*Repo {
	out := make([]*Repo, len(rows))
	for i, r := range rows {
		out[i] = r.repo()
	}
	return out
}

func (g *Gitea) listPlain(owner string) ([]*Repo, error) {
	base := ""
	for _, cand := range []string{"/orgs/" + owner + "/repos", "/users/" + owner + "/repos"} {
		r, err := g.get(cand, url.Values{"page": {"1"}, "limit": {"50"}})
		if err != nil {
			return nil, err
		}
		if r.OK() {
			base = cand
			break
		}
	}
	if base == "" {
		return nil, &httpx.APIError{Status: 404, Msg: "cannot list repos of '" + owner + "'"}
	}
	seen := map[int64]bool{}
	var out []*Repo
	for n := 1; ; n++ {
		q := url.Values{"page": {strconv.Itoa(n)}, "limit": {"50"}}
		r, err := g.get(base, q)
		if err != nil {
			return nil, err
		}
		if !r.OK() {
			return nil, httpx.Fail(r, "listing "+owner)
		}
		page, err := g.pages.decode(base+"?"+q.Encode(), r.Body, func(b []byte) ([]*Repo, error) {
			var rows []*giteaRepo
			if err := json.Unmarshal(b, &rows); err != nil {
				return nil, err
			}
			return convertGitea(rows), nil
		})
		if err != nil {
			return nil, err
		}
		added := 0
		for _, x := range page {
			if !seen[x.ID] {
				seen[x.ID] = true
				out = append(out, x)
				added++
			}
		}
		if added == 0 {
			return out, nil
		}
	}
}

func (g *Gitea) Get(owner, name string) (*Repo, error) {
	r, err := g.get("/repos/"+url.PathEscape(owner)+"/"+url.PathEscape(name), nil)
	if err != nil {
		return nil, err
	}
	if r.Status == 404 {
		return nil, nil
	}
	if !r.OK() {
		return nil, httpx.Fail(r, "")
	}
	var x giteaRepo
	if err := r.Decode(&x); err != nil {
		return nil, err
	}
	return x.repo(), nil
}

func (g *Gitea) CloneURL(r *Repo) string {
	return fmt.Sprintf("%s/%s/%s.git", g.cfg.SourceURL(), r.Owner, r.Name)
}

// Gogs wants basic auth as the token's owner instead of the token scheme
func (g *Gitea) GitHeader() (string, error) {
	if g.kind != "gogs" {
		return "Authorization: token " + g.cfg.Source.Token, nil
	}
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

func (g *Gitea) Hooks() Hooks { return &giteaHooks{g} }

func (g *Gitea) ParseWebhook(h http.Header, body []byte, secret string) (*Event, error) {
	signed := false
	for _, name := range []string{"X-Gitea-Signature", "X-Forgejo-Signature", "X-Gogs-Signature"} {
		if validMAC(secret, body, h.Get(name), "") {
			signed = true
		}
	}
	if !signed && !validMAC(secret, body, h.Get("X-Hub-Signature-256"), "sha256=") {
		return nil, ErrSignature
	}
	event := h.Get("X-Gitea-Event")
	if event == "" {
		event = h.Get("X-Forgejo-Event")
	}
	if event == "" {
		event = h.Get("X-Gogs-Event")
	}
	switch event {
	case "push", "create", "delete", "repository":
	default:
		return nil, nil
	}
	var p struct {
		Action     string `json:"action"`
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
	ev := &Event{Kind: Changed, Owner: owner, Name: name, ID: p.Repository.ID}
	if event == "repository" && p.Action == "deleted" {
		ev.Kind = Removed
	}
	return ev, nil
}

type giteaHooks struct{ g *Gitea }

func (h *giteaHooks) body() map[string]any {
	events, typ := giteaHookEvents, "gitea"
	if h.g.kind == "gogs" {
		events, typ = gogsHookEvents, "gogs"
	}
	return map[string]any{
		"type": typ, "active": true, "events": events,
		"config": map[string]string{
			"url": h.g.cfg.HookURL(), "content_type": "json", "secret": h.g.cfg.Listen.Secret},
	}
}

type hookInfo struct {
	ID     int64 `json:"id"`
	Config struct {
		URL string `json:"url"`
	} `json:"config"`
}

func (h *giteaHooks) ours(base string) ([]hookInfo, error) {
	r, err := h.g.get(base, nil)
	if err != nil {
		return nil, err
	}
	if !r.OK() {
		return nil, httpx.Fail(r, base)
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

func (h *giteaHooks) ensure(base string, force bool) (string, error) {
	mine, err := h.ours(base)
	if err != nil {
		return "", err
	}
	if len(mine) > 0 && !force {
		return "exists", nil
	}
	method, target, result := "POST", h.g.base+base, "created"
	if len(mine) > 0 {
		method, target, result = "PATCH", fmt.Sprintf("%s%s/%d", h.g.base, base, mine[0].ID), "updated"
	}
	r, err := h.g.http.Do(method, target, h.body())
	if err != nil {
		return "", err
	}
	if !r.OK() {
		return "", httpx.Fail(r, "")
	}
	return result, nil
}

// Forgejo never lists system webhooks (GET /admin/hooks is always empty): look ours up by id
func (h *giteaHooks) Global(known int64, force bool) (int64, string, error) {
	if known != 0 {
		r, err := h.g.get(fmt.Sprintf("/admin/hooks/%d", known), nil)
		if err != nil {
			return known, "", err
		}
		var x hookInfo
		if r.OK() && r.Decode(&x) == nil && x.Config.URL == h.g.cfg.HookURL() {
			if !force {
				return known, "exists", nil
			}
			p, err := h.g.http.Do("PATCH", fmt.Sprintf("%s/admin/hooks/%d", h.g.base, known), h.body())
			if err != nil {
				return known, "", err
			}
			if !p.OK() {
				return known, "", httpx.Fail(p, "")
			}
			return known, "updated", nil
		}
	}
	r, err := h.g.http.Do("POST", h.g.base+"/admin/hooks", h.body())
	if err != nil {
		return 0, "", err
	}
	if !r.OK() {
		return 0, "", httpx.Fail(r, "")
	}
	var created hookInfo
	if err := r.Decode(&created); err != nil {
		return 0, "", err
	}
	h.dropOlder(created.ID)
	return created.ID, "created", nil
}

func (h *giteaHooks) dropOlder(keep int64) {
	for id := keep - 1; id >= 1 && id > keep-1000; id-- {
		r, err := h.g.get(fmt.Sprintf("/admin/hooks/%d", id), nil)
		if err != nil || !r.OK() {
			continue
		}
		var x hookInfo
		if r.Decode(&x) == nil && x.Config.URL == h.g.cfg.HookURL() {
			if d, err := h.g.http.Do("DELETE", fmt.Sprintf("%s/admin/hooks/%d", h.g.base, id), nil); err == nil && d.OK() {
				logx.Infof("removed a duplicate system webhook (#%d)", id)
			}
		}
	}
}

func (h *giteaHooks) Repo(r *Repo, force bool) (string, error) {
	return h.ensure("/repos/"+url.PathEscape(r.Owner)+"/"+url.PathEscape(r.Name)+"/hooks", force)
}

func (h *giteaHooks) RemoveGlobal(id int64) bool {
	if id == 0 {
		return false
	}
	r, err := h.g.http.Do("DELETE", fmt.Sprintf("%s/admin/hooks/%d", h.g.base, id), nil)
	return err == nil && r.OK()
}

func (h *giteaHooks) RemoveRepo(r *Repo) int {
	base := "/repos/" + url.PathEscape(r.Owner) + "/" + url.PathEscape(r.Name) + "/hooks"
	mine, err := h.ours(base)
	if err != nil {
		return 0
	}
	n := 0
	for _, x := range mine {
		if d, err := h.g.http.Do("DELETE", fmt.Sprintf("%s%s/%d", h.g.base, base, x.ID), nil); err == nil && d.OK() {
			n++
		}
	}
	return n
}
