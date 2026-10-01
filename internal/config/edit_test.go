package config

import (
	"os"
	"path/filepath"
	"runtime"
	"strings"
	"testing"
)

const sample = `# my config
[source]
type = "forgejo"
url = "https://git.example.com"
token = "abcdefghijkl"

[github]
token = "ghp_abcdefghijkl"

[accounts]
alice = "alice-gh"

[listen]
secret = "0123456789abcdef"

[sync]
# how often
poll_interval = 5
visibility = true

[filter]
include = [
  "a",
  "b",
]
skip_forks = false
`

func sampleFile(t *testing.T) string {
	p := filepath.Join(t.TempDir(), "config.toml")
	if err := os.WriteFile(p, []byte(sample), 0o600); err != nil {
		t.Fatal(err)
	}
	return p
}

func TestSetValuesKeepsEverythingElse(t *testing.T) {
	p := sampleFile(t)
	err := SetValues(p, map[string]string{
		"sync.poll_interval": "30", "filter.new_repos": `"review"`, "filter.include": `["x"]`,
		"notify.url": `"https://ntfy.example.com/t"`,
	})
	if err != nil {
		t.Fatal(err)
	}
	b, _ := os.ReadFile(p)
	out := string(b)
	for _, want := range []string{"# my config", "# how often", "poll_interval = 30", "visibility = true",
		`new_repos = "review"`, `include = ["x"]`, "skip_forks = false", "[notify]", `url = "https://ntfy.example.com/t"`} {
		if !strings.Contains(out, want) {
			t.Errorf("missing %q in:\n%s", want, out)
		}
	}
	if strings.Contains(out, `"a"`) {
		t.Errorf("the old multi-line array was not replaced:\n%s", out)
	}
	st, _ := os.Stat(p)
	if runtime.GOOS != "windows" && st.Mode().Perm() != 0o600 {
		t.Errorf("permissions changed to %v", st.Mode().Perm())
	}
	c, err := Load(p)
	if err != nil || c.Filter.NewRepos != "review" || c.Sync.PollInterval != 30 {
		t.Fatalf("result does not load as expected: %v %+v", err, c)
	}
}

func TestSetValuesRefusesInvalid(t *testing.T) {
	p := sampleFile(t)
	err := SetValues(p, map[string]string{"filter.new_repos": `"maybe"`})
	if err == nil {
		t.Fatal("an invalid value must be refused")
	}
	b, _ := os.ReadFile(p)
	if string(b) != sample {
		t.Fatal("the file must be untouched after a refusal")
	}
}

func TestKeys(t *testing.T) {
	k, ok := FindKey("Sync.Poll_Interval")
	if !ok || k.Get(Default()) != "5" {
		t.Fatalf("lookup failed: %v %v", ok, k)
	}
	lit, err := k.Literal([]string{"abc"})
	if err == nil {
		t.Fatalf("a number key must refuse text, got %q", lit)
	}
	list, _ := mustKey(t, "filter.exclude").Literal([]string{"a, b", "c"})
	if list != `["a", "b", "c"]` {
		t.Fatalf("list literal: %s", list)
	}
	for _, p := range Profiles {
		for key := range p.Set {
			if _, ok := FindKey(key); !ok {
				t.Errorf("profile %s sets unknown key %s", p.Name, key)
			}
		}
	}
}

func mustKey(t *testing.T, n string) Key {
	k, ok := FindKey(n)
	if !ok {
		t.Fatalf("no key %s", n)
	}
	return k
}

func TestSetValuesKeepsOwner(t *testing.T) {
	if runtime.GOOS == "windows" || os.Geteuid() != 0 {
		t.Skip("needs root to hand a file to another user")
	}
	p := sampleFile(t)
	if err := os.Chown(p, 4242, 4343); err != nil {
		t.Skip(err)
	}
	if err := SetValues(p, map[string]string{"sync.poll_interval": "9"}); err != nil {
		t.Fatal(err)
	}
	st, _ := os.Stat(p)
	uid, gid := owner(st)
	if uid != 4242 || gid != 4343 {
		t.Fatalf("owner became %d:%d", uid, gid)
	}
}
