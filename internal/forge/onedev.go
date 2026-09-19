package forge

import (
	"crypto/subtle"
	"encoding/base64"
	"encoding/json"
	"fmt"
	"net/http"
	"strconv"
	"strings"
	"sync"

	"github.com/cristobaltormo/git-sync/internal/config"
	"github.com/cristobaltormo/git-sync/internal/httpx"
	"github.com/cristobaltormo/git-sync/internal/logx"
)

// projects live in a tree: the owner is the parent path, root projects use "/"
const OneDevRoot = "/"

type OneDev struct {
	cfg   *config.Config
	base  string
	http  *httpx.Client
	pages pageCache
	mu    sync.Mutex
	login string
}

type odProject struct {
	ID           int64   `json:"id"`
	Name         string  `json:"name"`
	Path         string  `json:"path"`
	Description  *string `json:"description"`
	ForkedFromID *int64  `json:"forkedFromId"`
}

func (p *odProject) repo() *Repo {
	owner := OneDevRoot
	if i := strings.LastIndex(p.Path, "/"); i >= 0 {
		owner = p.Path[:i]
	}
	return &Repo{
		ID: p.ID, Owner: owner, Name: p.Name, Private: true,
		Description: strPtr(p.Description), Fork: p.ForkedFromID != nil,
	}
}

func projectPath(owner, name string) string {
	if owner == OneDevRoot {
		return name
	}
	return owner + "/" + name
}

func newOneDev(cfg *config.Config) *OneDev {
	return &OneDev{
		cfg: cfg, base: cfg.SourceURL() + "/~api",
		http: httpx.New(map[string]string{
			"Authorization": "Bearer " + cfg.Source.Token,
			"Accept":        "application/json", "User-Agent": UserAgent,
		}),
	}
}

func (o *OneDev) Kind() string { return "onedev" }

func (o *OneDev) get(path string) (*httpx.Response, error) {
	return o.http.Do("GET", o.base+path, nil)
}

func (o *OneDev) Whoami() (*Identity, error) {
	r, err := o.get("/users/me")
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
		Name string `json:"name"`
	}
	if err := r.Decode(&u); err != nil {
		return nil, err
	}
	o.mu.Lock()
	o.login = u.Name
	o.mu.Unlock()
	return &Identity{Login: u.Name}, nil
}

func (o *OneDev) idOf(path string) (int64, bool, error) {
	r, err := o.get("/projects/ids/" + path)
	if err != nil {
		return 0, false, err
	}
	if !r.OK() {
		return 0, false, nil
	}
	id, err := strconv.ParseInt(strings.TrimSpace(string(r.Body)), 10, 64)
	return id, err == nil, nil
}

func (o *OneDev) OwnerExists(owner string) (bool, error) {
	if owner == OneDevRoot {
		return true, nil
	}
	_, ok, err := o.idOf(owner)
	return ok, err
}

func (o *OneDev) List(owner string) ([]*Repo, error) {
	var out []*Repo
	const count = 100
	for offset := 0; ; offset += count {
		r, err := o.get(fmt.Sprintf("/projects?offset=%d&count=%d", offset, count))
		if err != nil {
			return nil, err
		}
		if !r.OK() {
			return nil, httpx.Fail(r, "listing projects")
		}
		page, err := o.pages.decode(fmt.Sprintf("projects/%d", offset), r.Body, func(b []byte) ([]*Repo, error) {
			var rows []*odProject
			if err := json.Unmarshal(b, &rows); err != nil {
				return nil, err
			}
			repos := make([]*Repo, len(rows))
			for i, p := range rows {
				repos[i] = p.repo()
			}
			return repos, nil
		})
		if err != nil {
			return nil, err
		}
		for _, x := range page {
			if strings.EqualFold(x.Owner, owner) {
				out = append(out, x)
			}
		}
		if len(page) < count {
			return out, nil
		}
	}
}

func (o *OneDev) Get(owner, name string) (*Repo, error) {
	id, ok, err := o.idOf(projectPath(owner, name))
	if err != nil || !ok {
		return nil, err
	}
	return o.byID(id)
}

func (o *OneDev) byID(id int64) (*Repo, error) {
	r, err := o.get(fmt.Sprintf("/projects/%d", id))
	if err != nil {
		return nil, err
	}
	if r.Status == 404 {
		return nil, nil
	}
	if !r.OK() {
		return nil, httpx.Fail(r, "")
	}
	var p odProject
	if err := r.Decode(&p); err != nil {
		return nil, err
	}
	return p.repo(), nil
}

func (o *OneDev) CloneURL(r *Repo) string {
	return fmt.Sprintf("%s/%s.git", o.cfg.SourceURL(), projectPath(r.Owner, r.Name))
}

func (o *OneDev) GitHeader() (string, error) {
	o.mu.Lock()
	login := o.login
	o.mu.Unlock()
	if login == "" {
		id, err := o.Whoami()
		if err != nil {
			return "", err
		}
		login = id.Login
	}
	b := base64.StdEncoding.EncodeToString([]byte(login + ":" + o.cfg.Source.Token))
	logx.Hide(b)
	return "Authorization: Basic " + b, nil
}

// the secret arrives as is in X-OneDev-Signature and the payload only has the project id
func (o *OneDev) ParseWebhook(h http.Header, body []byte, secret string) (*Event, error) {
	if subtle.ConstantTimeCompare([]byte(h.Get("X-OneDev-Signature")), []byte(secret)) != 1 {
		return nil, ErrSignature
	}
	var p struct {
		ProjectID int64 `json:"projectId"`
	}
	if err := json.Unmarshal(body, &p); err != nil {
		return nil, err
	}
	if p.ProjectID == 0 {
		return nil, nil
	}
	repo, err := o.byID(p.ProjectID)
	if err != nil || repo == nil {
		return nil, err
	}
	return &Event{Kind: Changed, Owner: repo.Owner, Name: repo.Name, ID: repo.ID}, nil
}

func (o *OneDev) Hooks() Hooks { return &odHooks{o} }

type odHooks struct{ o *OneDev }

type odWebHook struct {
	PostURL    string   `json:"postUrl"`
	Secret     string   `json:"secret"`
	EventTypes []string `json:"eventTypes"`
	Headers    []any    `json:"headers"`
}

func (h *odHooks) setting(r *Repo) (int64, map[string]json.RawMessage, []odWebHook, error) {
	id, ok, err := h.o.idOf(projectPath(r.Owner, r.Name))
	if err != nil {
		return 0, nil, nil, err
	}
	if !ok {
		return 0, nil, nil, fmt.Errorf("project %s not found", r.Full())
	}
	resp, err := h.o.get(fmt.Sprintf("/projects/%d/setting", id))
	if err != nil {
		return 0, nil, nil, err
	}
	if !resp.OK() {
		return 0, nil, nil, httpx.Fail(resp, "")
	}
	var raw map[string]json.RawMessage
	if err := json.Unmarshal(resp.Body, &raw); err != nil {
		return 0, nil, nil, err
	}
	var hooks []odWebHook
	if v, ok := raw["webHooks"]; ok {
		json.Unmarshal(v, &hooks)
	}
	return id, raw, hooks, nil
}

func (h *odHooks) save(id int64, raw map[string]json.RawMessage, hooks []odWebHook) error {
	if hooks == nil {
		hooks = []odWebHook{}
	}
	b, _ := json.Marshal(hooks)
	raw["webHooks"] = b
	r, err := h.o.http.Do("POST", fmt.Sprintf("%s/projects/%d/setting", h.o.base, id), raw)
	if err != nil {
		return err
	}
	if !r.OK() {
		return httpx.Fail(r, "")
	}
	return nil
}

func (h *odHooks) Repo(r *Repo, force bool) (string, error) {
	id, raw, hooks, err := h.setting(r)
	if err != nil {
		return "", err
	}
	url := h.o.cfg.HookURL()
	entry := odWebHook{PostURL: url, Secret: h.o.cfg.Listen.Secret, EventTypes: []string{"CODE_PUSH"}, Headers: []any{}}
	for i, x := range hooks {
		if x.PostURL == url {
			if !force {
				return "exists", nil
			}
			hooks[i] = entry
			return "updated", h.save(id, raw, hooks)
		}
	}
	return "created", h.save(id, raw, append(hooks, entry))
}

func (h *odHooks) RemoveRepo(r *Repo) int {
	id, raw, hooks, err := h.setting(r)
	if err != nil {
		return 0
	}
	url := h.o.cfg.HookURL()
	var keep []odWebHook
	for _, x := range hooks {
		if x.PostURL != url {
			keep = append(keep, x)
		}
	}
	if len(keep) == len(hooks) || h.save(id, raw, keep) != nil {
		return 0
	}
	return len(hooks) - len(keep)
}

func (h *odHooks) Global(int64, bool) (int64, string, error) { return 0, "", errNoGlobalHooks }
func (h *odHooks) RemoveGlobal(int64) bool                   { return false }
