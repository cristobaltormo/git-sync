package config

import (
	"errors"
	"fmt"
	"net"
	"net/url"
	"os"
	"path/filepath"
	"regexp"
	"sort"
	"strconv"
	"strings"

	"github.com/BurntSushi/toml"
	"github.com/cristobaltormo/git-sync/internal/logx"
)

type Source struct {
	Type      string `toml:"type"`
	URL       string `toml:"url"`
	Token     string `toml:"token"`
	TokenFile string `toml:"token_file"`
	User      string `toml:"user"`
}

type GitHub struct {
	Token     string `toml:"token"`
	TokenFile string `toml:"token_file"`
	APIURL    string `toml:"api_url"`
	GitURL    string `toml:"git_url"`
}

type Listen struct {
	Host       string `toml:"host"`
	Port       int    `toml:"port"`
	Secret     string `toml:"secret"`
	SecretFile string `toml:"secret_file"`
	PublicURL  string `toml:"public_url"`
}

type Sync struct {
	PollInterval   int    `toml:"poll_interval"`
	VerifyInterval int    `toml:"verify_interval"`
	Workers        int    `toml:"workers"`
	Visibility     bool   `toml:"visibility"`
	Metadata       bool   `toml:"metadata"`
	DefaultBranch  bool   `toml:"default_branch"`
	Archived       bool   `toml:"archived"`
	Tags           bool   `toml:"tags"`
	OnDelete       string `toml:"on_delete"`
	DeleteGrace    int    `toml:"delete_grace"`
	DeleteLimit    int    `toml:"delete_limit"`
	AdoptExisting  bool   `toml:"adopt_existing"`
	DryRun         bool   `toml:"dry_run"`
	GitTimeout     int    `toml:"git_timeout"`
}

type Filter struct {
	Include      []string `toml:"include"`
	Exclude      []string `toml:"exclude"`
	Topic        string   `toml:"topic"`
	SkipForks    bool     `toml:"skip_forks"`
	SkipMirrors  bool     `toml:"skip_mirrors"`
	SkipArchived bool     `toml:"skip_archived"`
	SkipPrivate  bool     `toml:"skip_private"`
}

type Hooks struct {
	Mode string `toml:"mode"`
}

type Paths struct {
	StateDir string `toml:"state_dir"`
}

type Log struct {
	Level string `toml:"level"`
}

type Config struct {
	Source   Source            `toml:"source"`
	GitHub   GitHub            `toml:"github"`
	Accounts map[string]string `toml:"accounts"`
	Listen   Listen            `toml:"listen"`
	Sync     Sync              `toml:"sync"`
	Filter   Filter            `toml:"filter"`
	Hooks    Hooks             `toml:"hooks"`
	Paths    Paths             `toml:"paths"`
	Log      Log               `toml:"log"`

	Path         string `toml:"-"`
	lowerAccount map[string]string
}

var SourceTypes = []string{
	"forgejo", "gitea", "gogs", "codeberg", "gitlab", "gitbucket", "onedev",
	"bitbucket-cloud", "bitbucket-server",
}

var defaultURLs = map[string]string{
	"codeberg":        "https://codeberg.org",
	"gitlab":          "https://gitlab.com",
	"bitbucket-cloud": "https://bitbucket.org",
}

func DefaultURL(sourceType string) string { return defaultURLs[sourceType] }

type Error struct{ msg string }

func (e *Error) Error() string { return e.msg }

func Errorf(format string, a ...any) error { return &Error{fmt.Sprintf(format, a...)} }

func IsError(err error) bool {
	var ce *Error
	return errors.As(err, &ce)
}

func Default() *Config {
	return &Config{
		Source: Source{Type: "forgejo"},
		GitHub: GitHub{APIURL: "https://api.github.com", GitURL: "https://github.com"},
		Listen: Listen{Host: "127.0.0.1", Port: 9001},
		Sync: Sync{
			PollInterval: 5, VerifyInterval: 3600, Workers: 2,
			Visibility: true, Metadata: true, DefaultBranch: true, Archived: true, Tags: true,
			OnDelete: "delete", DeleteGrace: 10, DeleteLimit: 5, GitTimeout: 1800,
		},
		Filter: Filter{SkipMirrors: true},
		Hooks:  Hooks{Mode: "auto"},
		Log:    Log{Level: "info"},
	}
}

func DefaultStateDir() string {
	if d := os.Getenv("STATE_DIRECTORY"); d != "" {
		return strings.Split(d, ":")[0]
	}
	if os.Geteuid() == 0 {
		return "/var/lib/gitsync"
	}
	base := os.Getenv("XDG_STATE_HOME")
	if base == "" {
		home, _ := os.UserHomeDir()
		base = filepath.Join(home, ".local", "state")
	}
	return filepath.Join(base, "gitsync")
}

func fileExists(p string) bool {
	st, err := os.Stat(p)
	return err == nil && !st.IsDir()
}

func Find(explicit string) string {
	env := os.Getenv("GITSYNC_CONFIG")
	for _, c := range []string{explicit, env, "config.toml", "/etc/gitsync/config.toml"} {
		if c != "" && fileExists(c) {
			return c
		}
	}
	if explicit != "" {
		return explicit
	}
	if env != "" {
		return env
	}
	return "/etc/gitsync/config.toml"
}

func Load(path string) (*Config, error) {
	c := Default()
	md, err := toml.DecodeFile(path, c)
	if err != nil {
		if os.IsNotExist(err) {
			return nil, Errorf("config file not found: %s (run `gitsync init`)", path)
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
	c.Path = path
	if err := c.Finish(); err != nil {
		return nil, err
	}
	return c, nil
}

func readSecretFile(p string) (string, error) {
	b, err := os.ReadFile(p)
	if err != nil {
		return "", Errorf("cannot read %s: %v", p, errors.Unwrap(err))
	}
	return strings.TrimSpace(string(b)), nil
}

func (c *Config) Finish() error {
	overrides := []struct {
		env string
		dst *string
	}{
		{"GITSYNC_SOURCE_TOKEN", &c.Source.Token},
		{"GITSYNC_GITHUB_TOKEN", &c.GitHub.Token},
		{"GITSYNC_WEBHOOK_SECRET", &c.Listen.Secret},
	}
	for _, o := range overrides {
		if v := os.Getenv(o.env); v != "" {
			*o.dst = v
		}
	}
	files := []struct{ val, file *string }{
		{&c.Source.Token, &c.Source.TokenFile},
		{&c.GitHub.Token, &c.GitHub.TokenFile},
		{&c.Listen.Secret, &c.Listen.SecretFile},
	}
	for _, s := range files {
		if *s.val == "" && *s.file != "" {
			v, err := readSecretFile(*s.file)
			if err != nil {
				return err
			}
			*s.val = v
		}
	}
	if c.Source.URL == "" {
		c.Source.URL = defaultURLs[c.Source.Type]
	}
	if err := c.validate(); err != nil {
		return err
	}
	logx.Hide(c.Source.Token)
	logx.Hide(c.GitHub.Token)
	logx.Hide(c.Listen.Secret)
	logx.SetLevel(c.Log.Level)
	return nil
}

var httpURL = regexp.MustCompile(`^https?://[^/\s]+`)

func IsHTTPURL(s string) bool { return httpURL.MatchString(s) }

func oneOf(v string, opts ...string) bool {
	for _, o := range opts {
		if v == o {
			return true
		}
	}
	return false
}

func (c *Config) validate() error {
	if !oneOf(c.Source.Type, SourceTypes...) {
		return Errorf("source.type must be one of: %s", strings.Join(SourceTypes, ", "))
	}
	if !IsHTTPURL(c.Source.URL) {
		return Errorf(`source.url must be set, e.g. "https://git.example.com"`)
	}
	if c.Source.Token == "" {
		return Errorf("source.token is missing (or use token_file / GITSYNC_SOURCE_TOKEN)")
	}
	if c.GitHub.Token == "" {
		return Errorf("github.token is missing (or use token_file / GITSYNC_GITHUB_TOKEN)")
	}
	if len(c.Accounts) == 0 {
		return Errorf("[accounts] is empty: map at least one source owner to a GitHub owner")
	}
	c.lowerAccount = map[string]string{}
	for k, v := range c.Accounts {
		if v == "" {
			return Errorf("accounts.%s must be a GitHub user or organization", k)
		}
		c.lowerAccount[strings.ToLower(k)] = v
	}
	if c.Listen.Secret == "" {
		return Errorf("listen.secret is missing: webhooks are only accepted when signed (`gitsync init` generates one)")
	}
	if c.Listen.Port < 1 || c.Listen.Port > 65535 {
		return Errorf("listen.port must be a port number")
	}
	if !oneOf(c.Sync.OnDelete, "delete", "archive", "ignore") {
		return Errorf("sync.on_delete must be delete, archive or ignore")
	}
	if !oneOf(c.Hooks.Mode, "auto", "system", "repo", "none") {
		return Errorf("hooks.mode must be auto, system, repo or none")
	}
	for name, v := range map[string]int{
		"poll_interval": c.Sync.PollInterval, "verify_interval": c.Sync.VerifyInterval,
		"delete_grace": c.Sync.DeleteGrace, "delete_limit": c.Sync.DeleteLimit,
		"git_timeout": c.Sync.GitTimeout,
	} {
		if v < 0 {
			return Errorf("sync.%s must be 0 or more", name)
		}
	}
	if c.Sync.Workers < 1 {
		return Errorf("sync.workers must be at least 1")
	}
	if c.Paths.StateDir == "" {
		c.Paths.StateDir = DefaultStateDir()
	}
	return nil
}

func (c *Config) SourceURL() string { return strings.TrimRight(c.Source.URL, "/") }

func (c *Config) HookURL() string {
	if c.Listen.PublicURL != "" {
		return c.Listen.PublicURL
	}
	host := c.Listen.Host
	if host == "" || host == "0.0.0.0" || host == "::" {
		host = "127.0.0.1"
	}
	return "http://" + HostPort(host, c.Listen.Port) + "/hook"
}

func HostPort(host string, port int) string {
	return net.JoinHostPort(host, strconv.Itoa(port))
}

func (c *Config) GitHubOwner(sourceOwner string) (string, bool) {
	v, ok := c.lowerAccount[strings.ToLower(sourceOwner)]
	return v, ok
}

func (c *Config) SourceScope() string {
	u, err := url.Parse(c.SourceURL())
	if err != nil {
		return c.SourceURL() + "/"
	}
	return u.String() + "/"
}
