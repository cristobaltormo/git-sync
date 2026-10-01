package config

import (
	"os"
	"path/filepath"
	"strings"
	"testing"
)

func TestReposRoundTrip(t *testing.T) {
	p := filepath.Join(t.TempDir(), "repos.toml")
	r, err := LoadRepos(p)
	if err != nil {
		t.Fatal(err)
	}
	yes := true
	if err := r.Edit("My-Org/App", func(x *RepoRule) { x.Sync = &yes; x.Name = "app-mirror" }); err != nil {
		t.Fatal(err)
	}
	if err := r.Save(); err != nil {
		t.Fatal(err)
	}
	b, _ := os.ReadFile(p)
	if !strings.Contains(string(b), `[repos."my-org/app"]`) {
		t.Fatalf("unexpected file:\n%s", b)
	}
	again, err := LoadRepos(p)
	if err != nil {
		t.Fatal(err)
	}
	rule := again.Rule("my-org/APP")
	if rule == nil || rule.Sync == nil || !*rule.Sync || rule.Name != "app-mirror" {
		t.Fatalf("lost the rule: %+v", rule)
	}
	if err := again.Edit("my-org/app", func(x *RepoRule) { *x = RepoRule{} }); err != nil {
		t.Fatal(err)
	}
	if again.Rule("my-org/app") != nil {
		t.Fatal("an empty rule must be dropped")
	}
}

func TestReposSeveralRules(t *testing.T) {
	p := filepath.Join(t.TempDir(), "repos.toml")
	r, _ := LoadRepos(p)
	yes, no := true, false
	for name, v := range map[string]*bool{"a/one": &yes, "a/two": &no, "b/three": &yes} {
		if err := r.Edit(name, func(x *RepoRule) { x.Sync = v; x.Note = "n" }); err != nil {
			t.Fatal(err)
		}
	}
	if err := r.Save(); err != nil {
		t.Fatal(err)
	}
	again, err := LoadRepos(p)
	if err != nil {
		t.Fatalf("the file we wrote does not load: %v", err)
	}
	if len(again.Rules) != 3 || *again.Rule("a/two").Sync {
		t.Fatalf("lost rules: %+v", again.Rules)
	}
	b, _ := os.ReadFile(p)
	if strings.Count(string(b), "[repos]") != 0 {
		t.Fatalf("stray [repos] header:\n%s", b)
	}
}

func TestReposValidation(t *testing.T) {
	p := filepath.Join(t.TempDir(), "repos.toml")
	for name, body := range map[string]string{
		"bad name":      "[repos.\"a/b\"]\nname = \"no spaces\"\n",
		"bad on_delete": "[repos.\"a/b\"]\non_delete = \"explode\"\n",
		"unknown key":   "[repos.\"a/b\"]\nsyncc = true\n",
	} {
		os.WriteFile(p, []byte(body), 0o644)
		if _, err := LoadRepos(p); err == nil {
			t.Errorf("%s must be rejected", name)
		}
	}
}
