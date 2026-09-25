package mirror

import (
	"errors"
	"fmt"
	"net/http"
	"os"
	"os/exec"
	"path/filepath"
	"runtime"
	"strings"
	"testing"
	"time"

	"github.com/cristobaltormo/git-sync/internal/config"
	"github.com/cristobaltormo/git-sync/internal/forge"
)

type source struct{ header string }

func (s source) Kind() string                            { return "fake" }
func (s source) Whoami() (*forge.Identity, error)        { return nil, nil }
func (s source) List(string) ([]*forge.Repo, error)      { return nil, nil }
func (s source) Get(string, string) (*forge.Repo, error) { return nil, nil }
func (s source) OwnerExists(string) (bool, error)        { return true, nil }
func (s source) CloneURL(r *forge.Repo) string           { return "https://git.example.com/" + r.Full() + ".git" }
func (s source) GitHeader() (string, error)              { return s.header, nil }
func (s source) Hooks() forge.Hooks                      { return nil }
func (s source) ParseWebhook(_ http.Header, _ []byte, _ string) (*forge.Event, error) {
	return nil, nil
}

func TestCredentialsAreScopedHeaders(t *testing.T) {
	cfg := config.Default()
	cfg.Source.URL, cfg.Source.Token, cfg.GitHub.Token = "https://git.example.com", "src-token-123456", "gh-token-123456"
	g := New(cfg, source{header: "Authorization: token src-token-123456"}, false)
	for _, a := range g.opts {
		if strings.Contains(a, "token") {
			t.Errorf("no credentials on the command line: %s", a)
		}
	}
	env, err := g.Environment()
	if err != nil {
		t.Fatal(err)
	}
	joined := strings.Join(env, "\n")
	for _, want := range []string{
		"GIT_CONFIG_KEY_0=http.https://git.example.com/.extraheader",
		"GIT_CONFIG_VALUE_0=Authorization: token src-token-123456",
		"GIT_CONFIG_KEY_1=http.https://github.com/.extraheader",
	} {
		if !strings.Contains(joined, want) {
			t.Errorf("missing %q", want)
		}
	}
	for _, line := range env {
		if strings.HasPrefix(line, "GIT_CONFIG_VALUE_1=") &&
			(strings.Contains(line, "src-token-123456") || strings.Contains(line, "gh-token-123456")) {
			t.Errorf("GitHub's header must not carry the source token nor the token in clear: %s", line)
		}
	}
}

func TestMain(m *testing.M) {
	if mode := os.Getenv("GITSYNC_FAKE_GIT"); mode != "" {
		os.Exit(fakeGitMain(mode))
	}
	os.Exit(m.Run())
}

func fakeGitMain(mode string) int {
	switch mode {
	case "silent":
		time.Sleep(30 * time.Second)
	case "busy":
		for i := 1; i <= 8; i++ {
			fmt.Fprintf(os.Stderr, "Writing objects: %d\n", i)
			time.Sleep(100 * time.Millisecond)
		}
		fmt.Println("done")
	case "once":
		marker := os.Getenv("GITSYNC_FAKE_MARKER")
		if _, err := os.Stat(marker); err == nil {
			fmt.Println("ok")
			return 0
		}
		os.WriteFile(marker, nil, 0o600)
		time.Sleep(30 * time.Second)
	case "fail":
		fmt.Fprintln(os.Stderr, "one\ntwo\nfatal: nope")
		return 1
	}
	return 0
}

func fakeGit(t *testing.T, mode string) {
	t.Helper()
	self, err := os.Executable()
	if err != nil {
		t.Fatal(err)
	}
	blob, err := os.ReadFile(self)
	if err != nil {
		t.Fatal(err)
	}
	dir := t.TempDir()
	name := "git"
	if runtime.GOOS == "windows" {
		name = "git.exe"
	}
	if err := os.WriteFile(filepath.Join(dir, name), blob, 0o755); err != nil {
		t.Fatal(err)
	}
	t.Setenv("PATH", dir+string(os.PathListSeparator)+os.Getenv("PATH"))
	t.Setenv("GITSYNC_FAKE_GIT", mode)
}

func testGit() *Git {
	cfg := config.Default()
	g := New(cfg, source{}, false)
	g.stall = 1500 * time.Millisecond
	return g
}

func TestSilentGitIsKilled(t *testing.T) {
	fakeGit(t, "silent")
	start := time.Now()
	_, err := testGit().run(os.Environ(), "", "push")
	if !errors.Is(err, errStalled) || time.Since(start) > 5*time.Second {
		t.Fatalf("a silent git must be stopped quickly: %v after %v", err, time.Since(start))
	}
}

func TestBusyGitIsLeftAlone(t *testing.T) {
	fakeGit(t, "busy")
	out, err := testGit().run(os.Environ(), "", "push")
	if err != nil || strings.TrimSpace(out) != "done" {
		t.Fatalf("a git that reports progress is fine: %q %v", out, err)
	}
}

func TestStalledGitIsRetried(t *testing.T) {
	marker := filepath.Join(t.TempDir(), "seen")
	t.Setenv("GITSYNC_FAKE_MARKER", marker)
	fakeGit(t, "once")
	out, err := testGit().attempt(os.Environ(), "", "push")
	if err != nil || strings.TrimSpace(out) != "ok" {
		t.Fatalf("the second try should succeed: %q %v", out, err)
	}
}

func TestGitFailureShowsItsLastLines(t *testing.T) {
	fakeGit(t, "fail")
	_, err := testGit().run(os.Environ(), "", "fetch")
	if err == nil || !strings.Contains(err.Error(), "fatal: nope") {
		t.Fatalf("got %v", err)
	}
}

type local struct{ source }

func (l local) CloneURL(r *forge.Repo) string { return r.Website }
func (l local) GitHeader() (string, error)    { return "X-Test: 1", nil }

func gitIn(t *testing.T, dir string, args ...string) string {
	t.Helper()
	cmd := exec.Command("git", append([]string{"-c", "user.email=a@b.c", "-c", "user.name=t"}, args...)...)
	cmd.Dir = dir
	out, err := cmd.CombinedOutput()
	if err != nil {
		t.Fatalf("git %v: %v\n%s", args, err, out)
	}
	return strings.TrimSpace(string(out))
}

func TestMirrorPushesThenSkipsWhenNothingChanged(t *testing.T) {
	root := t.TempDir()
	srcDir, ghRoot := filepath.Join(root, "source"), filepath.Join(root, "gh")
	if err := os.MkdirAll(filepath.Join(ghRoot, "alice"), 0o755); err != nil {
		t.Fatal(err)
	}
	gitIn(t, root, "init", "-q", "-b", "main", srcDir)
	gitIn(t, srcDir, "commit", "-q", "--allow-empty", "-m", "one")
	gitIn(t, root, "init", "-q", "--bare", filepath.Join(ghRoot, "alice", "site.git"))

	cfg := config.Default()
	cfg.Paths.StateDir = filepath.Join(root, "state")
	cfg.GitHub.Token, cfg.GitHub.GitURL = "gh-token-123456", "file://"+ghRoot
	g := New(cfg, local{}, false)
	repo := &forge.Repo{ID: 7, Owner: "alice", Name: "site", Website: srcDir}

	first, err := g.Mirror(repo, "alice", "site", "")
	if err != nil || first.Summary != "1 ref(s) updated" || first.Refs == "" {
		t.Fatalf("first push: %+v %v", first, err)
	}
	want := gitIn(t, srcDir, "rev-parse", "main")
	if got := gitIn(t, filepath.Join(ghRoot, "alice", "site.git"), "rev-parse", "main"); got != want {
		t.Fatalf("destination is at %s, source at %s", got, want)
	}

	// nothing new: no push at all, even if the destination were unreachable
	os.RemoveAll(filepath.Join(ghRoot, "alice", "site.git"))
	again, err := g.Mirror(repo, "alice", "site", first.Refs)
	if err != nil || again.Summary != "up to date" || again.Refs != first.Refs {
		t.Fatalf("second run: %+v %v", again, err)
	}

	gitIn(t, srcDir, "commit", "-q", "--allow-empty", "-m", "two")
	gitIn(t, root, "init", "-q", "--bare", filepath.Join(ghRoot, "alice", "site.git"))
	next, err := g.Mirror(repo, "alice", "site", first.Refs)
	if err != nil || next.Refs == first.Refs || next.Summary == "up to date" {
		t.Fatalf("a new commit must be pushed: %+v %v", next, err)
	}
}
