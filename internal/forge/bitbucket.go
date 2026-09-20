package forge

import (
	"encoding/base64"
	"encoding/json"
	"fmt"
	"hash/fnv"
	"net/http"
	"net/url"
	"strings"
	"sync"

	"github.com/cristobaltormo/git-sync/internal/config"
	"github.com/cristobaltormo/git-sync/internal/httpx"
	"github.com/cristobaltormo/git-sync/internal/logx"
)

// Experimental: written from the API documentation, tested against a simulated server only.
type BitbucketCloud struct {
	cfg   *config.Config
	base  string
	http  *httpx.Client
	pages pageCache
	mu    sync.Mutex
}

type bbRepo struct {
	UUID        string `json:"uuid"`
	Slug        string `json:"slug"`
	FullName    string `json:"full_name"`
	IsPrivate   bool   `json:"is_private"`
	Description string `json:"description"`
	Website     string `json:"website"`
	UpdatedOn   string `json:"updated_on"`
	Parent      any    `json:"parent"`
	MainBranch  *struct {
		Name string `json:"name"`
	} `json:"mainbranch"`
}

func hashID(s string) int64 {
	h := fnv.New64a()
	h.Write([]byte(s))
	return int64(h.Sum64() >> 1)
}

func (b *bbRepo) repo() *Repo {
	owner, _, _ := strings.Cut(b.FullName, "/")
	r := &Repo{
		ID: hashID(b.UUID), Owner: owner, Name: b.Slug, Private: b.IsPrivate,
		Description: strings.TrimSpace(b.Description), Website: strings.TrimSpace(b.Website),
		Fork: b.Parent != nil, Updated: b.UpdatedOn, Empty: b.MainBranch == nil,
	}
	if b.MainBranch != nil {
		r.DefaultBranch = b.MainBranch.Name
	}
	return r
}

func newBitbucketCloud(cfg *config.Config) *BitbucketCloud {
	base := cfg.SourceURL() + "/2.0"
	if cfg.SourceURL() == "https://bitbucket.org" {
		base = "https://api.bitbucket.org/2.0"
	}
	return &BitbucketCloud{
		cfg: cfg, base: base,
		http: httpx.New(map[string]string{
			"Authorization": "Bearer " + cfg.Source.Token,
			"Accept":        "application/json", "User-Agent": UserAgent,
		}),
	}
}

func (b *BitbucketCloud) Kind() string { return "bitbucket-cloud" }

func (b *BitbucketCloud) Whoami() (*Identity, error) {
	r, err := b.http.Do("GET", b.base+"/user", nil)
	if err != nil {
		return nil, err
	}
	if r.OK() {
		var u struct {
			Nickname string `json:"nickname"`
			Username string `json:"username"`
		}
		if err := r.Decode(&u); err != nil {
			return nil, err
		}
		login := u.Nickname
		if login == "" {
			login = u.Username
		}
		return &Identity{Login: login}, nil
	}
	// repository tokens cannot read /user, any listing proves the token works
	r, err = b.http.Do("GET", b.base+"/repositories?role=member&pagelen=1", nil)
	if err != nil {
		return nil, err
	}
	if !r.OK() {
		return nil, &httpx.APIError{Status: r.Status, Msg: "the source token was rejected"}
	}
	return &Identity{Login: "token"}, nil
}

func (b *BitbucketCloud) OwnerExists(owner string) (bool, error) {
	r, err := b.http.Do("GET", b.base+"/workspaces/"+url.PathEscape(owner), nil)
	if err != nil {
		return false, err
	}
	return r.OK(), nil
}

func (b *BitbucketCloud) List(owner string) ([]*Repo, error) {
	next := b.base + "/repositories/" + url.PathEscape(owner) + "?pagelen=100"
	var out []*Repo
	for n := 0; next != ""; n++ {
		r, err := b.http.Do("GET", next, nil)
		if err != nil {
			return nil, err
		}
		if !r.OK() {
			return nil, httpx.Fail(r, "listing "+owner)
		}
		var page struct {
			Values []*bbRepo `json:"values"`
			Next   string    `json:"next"`
		}
		if err := r.Decode(&page); err != nil {
			return nil, err
		}
		for _, v := range page.Values {
			out = append(out, v.repo())
		}
		next = page.Next
	}
	return out, nil
}

func (b *BitbucketCloud) Get(owner, name string) (*Repo, error) {
	r, err := b.http.Do("GET", b.base+"/repositories/"+url.PathEscape(owner)+"/"+url.PathEscape(name), nil)
	if err != nil {
		return nil, err
	}
	if r.Status == 404 {
		return nil, nil
	}
	if !r.OK() {
		return nil, httpx.Fail(r, "")
	}
	var x bbRepo
	if err := r.Decode(&x); err != nil {
		return nil, err
	}
	return x.repo(), nil
}

func (b *BitbucketCloud) CloneURL(r *Repo) string {
	return fmt.Sprintf("%s/%s/%s.git", b.cfg.SourceURL(), r.Owner, r.Name)
}

func (b *BitbucketCloud) GitHeader() (string, error) {
	v := base64.StdEncoding.EncodeToString([]byte("x-token-auth:" + b.cfg.Source.Token))
	logx.Hide(v)
	return "Authorization: Basic " + v, nil
}

func (b *BitbucketCloud) ParseWebhook(h http.Header, body []byte, secret string) (*Event, error) {
	if !validMAC(secret, body, h.Get("X-Hub-Signature"), "sha256=") {
		return nil, ErrSignature
	}
	if !strings.HasPrefix(h.Get("X-Event-Key"), "repo:") {
		return nil, nil
	}
	var p struct {
		Repository struct {
			UUID     string `json:"uuid"`
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
	return &Event{Kind: Changed, Owner: owner, Name: name, ID: hashID(p.Repository.UUID)}, nil
}

func (b *BitbucketCloud) Hooks() Hooks { return &bbHooks{b} }

type bbHooks struct{ b *BitbucketCloud }

func (h *bbHooks) path(r *Repo) string {
	return "/repositories/" + url.PathEscape(r.Owner) + "/" + url.PathEscape(r.Name) + "/hooks"
}

func (h *bbHooks) body() map[string]any {
	return map[string]any{
		"description": "gitsync", "url": h.b.cfg.HookURL(), "active": true,
		"secret": h.b.cfg.Listen.Secret, "events": []string{"repo:push", "repo:updated"},
	}
}

type bbHook struct {
	UUID string `json:"uuid"`
	URL  string `json:"url"`
}

func (h *bbHooks) ours(r *Repo) ([]bbHook, error) {
	resp, err := h.b.http.Do("GET", h.b.base+h.path(r)+"?pagelen=100", nil)
	if err != nil {
		return nil, err
	}
	if !resp.OK() {
		return nil, httpx.Fail(resp, "")
	}
	var page struct {
		Values []bbHook `json:"values"`
	}
	if err := resp.Decode(&page); err != nil {
		return nil, err
	}
	var mine []bbHook
	for _, x := range page.Values {
		if x.URL == h.b.cfg.HookURL() {
			mine = append(mine, x)
		}
	}
	return mine, nil
}

func (h *bbHooks) Repo(r *Repo, force bool) (string, error) {
	mine, err := h.ours(r)
	if err != nil {
		return "", err
	}
	if len(mine) > 0 && !force {
		return "exists", nil
	}
	method, target, result := "POST", h.b.base+h.path(r), "created"
	if len(mine) > 0 {
		method, target, result = "PUT", h.b.base+h.path(r)+"/"+url.PathEscape(mine[0].UUID), "updated"
	}
	resp, err := h.b.http.Do(method, target, h.body())
	if err != nil {
		return "", err
	}
	if !resp.OK() {
		return "", httpx.Fail(resp, "")
	}
	return result, nil
}

func (h *bbHooks) RemoveRepo(r *Repo) int {
	mine, err := h.ours(r)
	if err != nil {
		return 0
	}
	n := 0
	for _, x := range mine {
		if d, err := h.b.http.Do("DELETE", h.b.base+h.path(r)+"/"+url.PathEscape(x.UUID), nil); err == nil && d.OK() {
			n++
		}
	}
	return n
}

func (h *bbHooks) Global(int64, bool) (int64, string, error) { return 0, "", errNoGlobalHooks }
func (h *bbHooks) RemoveGlobal(int64) bool                   { return false }

// Experimental, like the Cloud provider. Owners are project keys.
type BitbucketServer struct {
	cfg  *config.Config
	base string
	http *httpx.Client
}

type bsRepo struct {
	ID          int64  `json:"id"`
	Slug        string `json:"slug"`
	Description string `json:"description"`
	Public      bool   `json:"public"`
	Archived    bool   `json:"archived"`
	Origin      any    `json:"origin"`
	Project     struct {
		Key string `json:"key"`
	} `json:"project"`
}

func (b *bsRepo) repo() *Repo {
	return &Repo{
		ID: b.ID, Owner: b.Project.Key, Name: b.Slug, Private: !b.Public,
		Description: strings.TrimSpace(b.Description), Archived: b.Archived, Fork: b.Origin != nil,
	}
}

func newBitbucketServer(cfg *config.Config) *BitbucketServer {
	return &BitbucketServer{
		cfg: cfg, base: cfg.SourceURL() + "/rest/api/1.0",
		http: httpx.New(map[string]string{
			"Authorization": "Bearer " + cfg.Source.Token,
			"Accept":        "application/json", "User-Agent": UserAgent,
		}),
	}
}

func (b *BitbucketServer) Kind() string { return "bitbucket-server" }

func (b *BitbucketServer) Whoami() (*Identity, error) {
	r, err := b.http.Do("GET", b.cfg.SourceURL()+"/plugins/servlet/applinks/whoami", nil)
	if err != nil {
		return nil, err
	}
	if r.Status == 401 {
		return nil, &httpx.APIError{Status: 401, Msg: "the source token was rejected"}
	}
	if !r.OK() {
		return nil, httpx.Fail(r, "")
	}
	return &Identity{Login: strings.TrimSpace(string(r.Body))}, nil
}

func (b *BitbucketServer) OwnerExists(owner string) (bool, error) {
	r, err := b.http.Do("GET", b.base+"/projects/"+url.PathEscape(owner), nil)
	if err != nil {
		return false, err
	}
	return r.OK(), nil
}

func (b *BitbucketServer) List(owner string) ([]*Repo, error) {
	var out []*Repo
	for start := 0; ; {
		r, err := b.http.Do("GET", fmt.Sprintf("%s/projects/%s/repos?limit=100&start=%d", b.base, url.PathEscape(owner), start), nil)
		if err != nil {
			return nil, err
		}
		if !r.OK() {
			return nil, httpx.Fail(r, "listing "+owner)
		}
		var page struct {
			Values        []*bsRepo `json:"values"`
			IsLastPage    bool      `json:"isLastPage"`
			NextPageStart int       `json:"nextPageStart"`
		}
		if err := r.Decode(&page); err != nil {
			return nil, err
		}
		for _, v := range page.Values {
			out = append(out, v.repo())
		}
		if page.IsLastPage {
			return out, nil
		}
		start = page.NextPageStart
	}
}

func (b *BitbucketServer) Get(owner, name string) (*Repo, error) {
	r, err := b.http.Do("GET", b.base+"/projects/"+url.PathEscape(owner)+"/repos/"+url.PathEscape(name), nil)
	if err != nil {
		return nil, err
	}
	if r.Status == 404 {
		return nil, nil
	}
	if !r.OK() {
		return nil, httpx.Fail(r, "")
	}
	var x bsRepo
	if err := r.Decode(&x); err != nil {
		return nil, err
	}
	return x.repo(), nil
}

func (b *BitbucketServer) CloneURL(r *Repo) string {
	return fmt.Sprintf("%s/scm/%s/%s.git", b.cfg.SourceURL(), strings.ToLower(r.Owner), r.Name)
}

func (b *BitbucketServer) GitHeader() (string, error) {
	return "Authorization: Bearer " + b.cfg.Source.Token, nil
}

func (b *BitbucketServer) ParseWebhook(h http.Header, body []byte, secret string) (*Event, error) {
	if !validMAC(secret, body, h.Get("X-Hub-Signature"), "sha256=") {
		return nil, ErrSignature
	}
	if !strings.HasPrefix(h.Get("X-Event-Key"), "repo:") {
		return nil, nil
	}
	var p struct {
		Repository struct {
			ID      int64  `json:"id"`
			Slug    string `json:"slug"`
			Project struct {
				Key string `json:"key"`
			} `json:"project"`
		} `json:"repository"`
	}
	if err := json.Unmarshal(body, &p); err != nil {
		return nil, err
	}
	if p.Repository.Slug == "" {
		return nil, nil
	}
	return &Event{Kind: Changed, Owner: p.Repository.Project.Key, Name: p.Repository.Slug, ID: p.Repository.ID}, nil
}

func (b *BitbucketServer) Hooks() Hooks { return &bsHooks{b} }

type bsHooks struct{ b *BitbucketServer }

func (h *bsHooks) path(r *Repo) string {
	return "/projects/" + url.PathEscape(r.Owner) + "/repos/" + url.PathEscape(r.Name) + "/webhooks"
}

func (h *bsHooks) body() map[string]any {
	return map[string]any{
		"name": "gitsync", "url": h.b.cfg.HookURL(), "active": true,
		"events":        []string{"repo:refs_changed", "repo:modified"},
		"configuration": map[string]string{"secret": h.b.cfg.Listen.Secret},
	}
}

type bsHook struct {
	ID  int64  `json:"id"`
	URL string `json:"url"`
}

func (h *bsHooks) ours(r *Repo) ([]bsHook, error) {
	resp, err := h.b.http.Do("GET", h.b.base+h.path(r)+"?limit=100", nil)
	if err != nil {
		return nil, err
	}
	if !resp.OK() {
		return nil, httpx.Fail(resp, "")
	}
	var page struct {
		Values []bsHook `json:"values"`
	}
	if err := resp.Decode(&page); err != nil {
		return nil, err
	}
	var mine []bsHook
	for _, x := range page.Values {
		if x.URL == h.b.cfg.HookURL() {
			mine = append(mine, x)
		}
	}
	return mine, nil
}

func (h *bsHooks) Repo(r *Repo, force bool) (string, error) {
	mine, err := h.ours(r)
	if err != nil {
		return "", err
	}
	if len(mine) > 0 && !force {
		return "exists", nil
	}
	method, target, result := "POST", h.b.base+h.path(r), "created"
	if len(mine) > 0 {
		method, target, result = "PUT", fmt.Sprintf("%s%s/%d", h.b.base, h.path(r), mine[0].ID), "updated"
	}
	resp, err := h.b.http.Do(method, target, h.body())
	if err != nil {
		return "", err
	}
	if !resp.OK() {
		return "", httpx.Fail(resp, "")
	}
	return result, nil
}

func (h *bsHooks) RemoveRepo(r *Repo) int {
	mine, err := h.ours(r)
	if err != nil {
		return 0
	}
	n := 0
	for _, x := range mine {
		if d, err := h.b.http.Do("DELETE", fmt.Sprintf("%s%s/%d", h.b.base, h.path(r), x.ID), nil); err == nil && d.OK() {
			n++
		}
	}
	return n
}

func (h *bsHooks) Global(int64, bool) (int64, string, error) { return 0, "", errNoGlobalHooks }
func (h *bsHooks) RemoveGlobal(int64) bool                   { return false }
