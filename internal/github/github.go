package github

import (
	"fmt"
	"regexp"
	"sort"
	"strings"
	"sync"

	"github.com/cristobaltormo/git-sync/internal/config"
	"github.com/cristobaltormo/git-sync/internal/forge"
	"github.com/cristobaltormo/git-sync/internal/httpx"
	"github.com/cristobaltormo/git-sync/internal/logx"
)

type Repo struct {
	Name  string `json:"name"`
	Owner struct {
		Login string `json:"login"`
	} `json:"owner"`
	Private       bool     `json:"private"`
	Archived      bool     `json:"archived"`
	Description   *string  `json:"description"`
	Homepage      *string  `json:"homepage"`
	DefaultBranch string   `json:"default_branch"`
	Topics        []string `json:"topics"`
}

type Snap struct {
	Private       bool     `json:"private"`
	Archived      bool     `json:"archived"`
	Description   string   `json:"description"`
	Homepage      string   `json:"homepage"`
	DefaultBranch string   `json:"default_branch"`
	Topics        []string `json:"topics"`
}

func text(s *string) string {
	if s == nil {
		return ""
	}
	return strings.TrimSpace(*s)
}

func Snapshot(r *Repo) Snap {
	t := append([]string(nil), r.Topics...)
	sort.Strings(t)
	return Snap{Private: r.Private, Archived: r.Archived, Description: text(r.Description),
		Homepage: text(r.Homepage), DefaultBranch: r.DefaultBranch, Topics: t}
}

func (s Snap) Clone() Snap {
	s.Topics = append([]string(nil), s.Topics...)
	return s
}

var topicJunk = regexp.MustCompile(`[^a-z0-9-]+`)

func CleanTopics(topics []string, drop string) []string {
	var out []string
	seen := map[string]bool{}
	for _, t := range topics {
		t = strings.ReplaceAll(strings.ToLower(t), "_", "-")
		t = strings.Trim(topicJunk.ReplaceAllString(t, "-"), "-")
		if len(t) > 50 {
			t = t[:50]
		}
		if t == "" || t == drop || seen[t] {
			continue
		}
		seen[t] = true
		out = append(out, t)
	}
	if len(out) > 20 {
		out = out[:20]
	}
	return out
}

type Client struct {
	dry   bool
	base  string
	http  *httpx.Client
	mu    sync.Mutex
	me    string
	scope *string
	types map[string]string
}

func New(cfg *config.Config, dry bool) *Client {
	return &Client{
		dry: dry, base: strings.TrimRight(cfg.GitHub.APIURL, "/"),
		http: httpx.New(map[string]string{
			"Authorization":        "Bearer " + cfg.GitHub.Token,
			"Accept":               "application/vnd.github+json",
			"X-GitHub-Api-Version": "2022-11-28", "User-Agent": forge.UserAgent,
		}),
		types: map[string]string{},
	}
}

func (c *Client) HTTP() *httpx.Client { return c.http }

func (c *Client) Whoami() (string, *string, error) {
	c.mu.Lock()
	if c.me != "" {
		defer c.mu.Unlock()
		return c.me, c.scope, nil
	}
	c.mu.Unlock()
	r, err := c.http.Do("GET", c.base+"/user", nil)
	if err != nil {
		return "", nil, err
	}
	if r.Status == 401 {
		return "", nil, &httpx.APIError{Status: 401, Msg: "the GitHub token was rejected"}
	}
	if !r.OK() {
		return "", nil, httpx.Fail(r, "")
	}
	var u struct {
		Login string `json:"login"`
	}
	if err := r.Decode(&u); err != nil {
		return "", nil, err
	}
	var scope *string
	if v, ok := r.Header["X-Oauth-Scopes"]; ok && len(v) > 0 {
		scope = &v[0]
	}
	c.mu.Lock()
	c.me, c.scope = u.Login, scope
	c.mu.Unlock()
	return u.Login, scope, nil
}

func (c *Client) OwnerType(login string) (string, error) {
	c.mu.Lock()
	t, ok := c.types[login]
	c.mu.Unlock()
	if ok {
		return t, nil
	}
	r, err := c.http.Do("GET", c.base+"/users/"+login, nil)
	if err != nil {
		return "", err
	}
	if !r.OK() {
		return "", &httpx.APIError{Status: r.Status, Msg: "GitHub account '" + login + "' not found"}
	}
	var u struct {
		Type string `json:"type"`
	}
	if err := r.Decode(&u); err != nil {
		return "", err
	}
	c.mu.Lock()
	c.types[login] = u.Type
	c.mu.Unlock()
	return u.Type, nil
}

func (c *Client) Get(owner, name string) (*Repo, error) {
	r, err := c.http.Do("GET", fmt.Sprintf("%s/repos/%s/%s", c.base, owner, name), nil)
	if err != nil {
		return nil, err
	}
	if r.Status == 404 {
		return nil, nil
	}
	if !r.OK() {
		return nil, httpx.Fail(r, "")
	}
	var x Repo
	if err := r.Decode(&x); err != nil {
		return nil, err
	}
	// renamed and deleted repos leave a redirect from their old name
	if !strings.EqualFold(x.Name, name) || (x.Owner.Login != "" && !strings.EqualFold(x.Owner.Login, owner)) {
		return nil, nil
	}
	return &x, nil
}

func (c *Client) send(method, path string, body any) (*httpx.Response, error) {
	if c.dry {
		logx.Infof("dry-run: %s %s %v", method, path, body)
		return nil, nil
	}
	r, err := c.http.Do(method, c.base+path, body)
	if err != nil {
		return nil, err
	}
	if !r.OK() {
		return nil, httpx.Fail(r, "")
	}
	return r, nil
}

func (c *Client) Create(owner, name, description, homepage string) (*Repo, error) {
	body := map[string]any{"name": name, "private": true, "has_wiki": false, "has_projects": false}
	if description != "" {
		body["description"] = description
	}
	if homepage != "" {
		body["homepage"] = homepage
	}
	me, _, err := c.Whoami()
	if err != nil {
		return nil, err
	}
	path := "/user/repos"
	if !strings.EqualFold(owner, me) {
		t, err := c.OwnerType(owner)
		if err != nil {
			return nil, err
		}
		if t != "Organization" {
			return nil, &httpx.APIError{Status: 403, Msg: fmt.Sprintf(
				"the token belongs to '%s' and cannot create repos under the user '%s'", me, owner)}
		}
		path = "/orgs/" + owner + "/repos"
	}
	r, err := c.send("POST", path, body)
	if err != nil || r == nil {
		return nil, err
	}
	var x Repo
	if err := r.Decode(&x); err != nil {
		return nil, err
	}
	return &x, nil
}

func (c *Client) Patch(owner, name string, fields map[string]any) error {
	_, err := c.send("PATCH", fmt.Sprintf("/repos/%s/%s", owner, name), fields)
	return err
}

func (c *Client) SetTopics(owner, name string, topics []string) error {
	if topics == nil {
		topics = []string{}
	}
	_, err := c.send("PUT", fmt.Sprintf("/repos/%s/%s/topics", owner, name), map[string]any{"names": topics})
	return err
}

func (c *Client) Delete(owner, name string) error {
	_, err := c.send("DELETE", fmt.Sprintf("/repos/%s/%s", owner, name), nil)
	return err
}
