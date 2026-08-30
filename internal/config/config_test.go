package config

import (
	"os"
	"path/filepath"
	"strings"
	"testing"
)

const minimal = `
[source]
url = "https://git.example.com"
token = "src-token-123456"
[github]
token = "gh-token-123456"
[accounts]
alice = "alice-gh"
[listen]
secret = "s3cret-value-123"
`

func write(t *testing.T, body string) string {
	p := filepath.Join(t.TempDir(), "config.toml")
	if err := os.WriteFile(p, []byte(body), 0o600); err != nil {
		t.Fatal(err)
	}
	return p
}

func TestDefaults(t *testing.T) {
	c, err := Load(write(t, minimal))
	if err != nil {
		t.Fatal(err)
	}
	if c.Sync.PollInterval != 5 || !c.Sync.Visibility || c.Sync.OnDelete != "delete" {
		t.Fatalf("unexpected defaults: %+v", c.Sync)
	}
	if got := c.HookURL(); got != "http://127.0.0.1:9001/hook" {
		t.Fatalf("hook url: %s", got)
	}
	if gh, ok := c.GitHubOwner("ALICE"); !ok || gh != "alice-gh" {
		t.Fatal("accounts must be case-insensitive")
	}
}

func TestOverridesKeepOtherDefaults(t *testing.T) {
	c, err := Load(write(t, minimal+"\n[sync]\npoll_interval = 2\nvisibility = false\n"))
	if err != nil {
		t.Fatal(err)
	}
	if c.Sync.PollInterval != 2 || c.Sync.Visibility || !c.Sync.Metadata {
		t.Fatalf("got %+v", c.Sync)
	}
}

func TestErrors(t *testing.T) {
	cases := map[string]string{
		"unknown option": minimal + "\n[sync]\nnope = 1\n",
		"source.token":   strings.Replace(minimal, `token = "src-token-123456"`, "", 1),
		"listen.secret":  strings.Replace(minimal, `secret = "s3cret-value-123"`, "", 1),
		"on_delete":      minimal + "\n[sync]\non_delete = \"explode\"\n",
		"workers":        minimal + "\n[sync]\nworkers = 0\n",
		"source.url":     "[source]\ntype = \"forgejo\"\nurl = \"nope\"\n",
		"source.type":    strings.Replace(minimal, "[source]", "[source]\ntype = \"cvs\"", 1),
	}
	for want, body := range cases {
		_, err := Load(write(t, body))
		if err == nil || !strings.Contains(err.Error(), want) {
			t.Errorf("want error containing %q, got %v", want, err)
		} else if !IsError(err) {
			t.Errorf("%q should be a config error", want)
		}
	}
}

func TestMissingFileSuggestsInit(t *testing.T) {
	_, err := Load("/nonexistent/x.toml")
	if err == nil || !strings.Contains(err.Error(), "init") {
		t.Fatalf("got %v", err)
	}
}

func TestSecretsFromFileAndEnvironment(t *testing.T) {
	tokFile := filepath.Join(t.TempDir(), "tok")
	os.WriteFile(tokFile, []byte("from-file-token\n"), 0o600)
	body := strings.Replace(minimal, `token = "src-token-123456"`, `token_file = "`+tokFile+`"`, 1)
	c, err := Load(write(t, body))
	if err != nil || c.Source.Token != "from-file-token" {
		t.Fatalf("file: %v %v", c, err)
	}
	t.Setenv("GITSYNC_SOURCE_TOKEN", "from-env-token")
	c, err = Load(write(t, body))
	if err != nil || c.Source.Token != "from-env-token" {
		t.Fatalf("env: %v %v", c, err)
	}
}

func TestDefaultURLs(t *testing.T) {
	for typ, want := range map[string]string{
		"gitlab": "https://gitlab.com", "codeberg": "https://codeberg.org", "bitbucket-cloud": "https://bitbucket.org",
	} {
		body := strings.Replace(strings.Replace(minimal, `url = "https://git.example.com"`, "", 1), "[source]", "[source]\ntype = \""+typ+"\"", 1)
		c, err := Load(write(t, body))
		if err != nil || c.Source.URL != want {
			t.Errorf("%s: got %v %v", typ, c, err)
		}
	}
}

func TestHookURL(t *testing.T) {
	c := Default()
	c.Listen.Host = "0.0.0.0"
	if c.HookURL() != "http://127.0.0.1:9001/hook" {
		t.Fatal(c.HookURL())
	}
	c.Listen.PublicURL = "https://sync.example.com/hook"
	if c.HookURL() != "https://sync.example.com/hook" {
		t.Fatal(c.HookURL())
	}
	c.Listen.PublicURL, c.Listen.Host = "", "::1"
	if c.HookURL() != "http://[::1]:9001/hook" {
		t.Fatal(c.HookURL())
	}
}
