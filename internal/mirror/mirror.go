package mirror

import (
	"bytes"
	"context"
	"crypto/sha1"
	"encoding/base64"
	"encoding/hex"
	"errors"
	"fmt"
	"os"
	"os/exec"
	"path/filepath"
	"strings"
	"sync"
	"sync/atomic"
	"time"

	"github.com/cristobaltormo/git-sync/internal/config"
	"github.com/cristobaltormo/git-sync/internal/forge"
	"github.com/cristobaltormo/git-sync/internal/logx"
)

type Git struct {
	cfg   *config.Config
	src   forge.Provider
	dry   bool
	root  string
	opts  []string
	stall time.Duration
}

func New(cfg *config.Config, src forge.Provider, dry bool) *Git {
	logx.Hide(base64.StdEncoding.EncodeToString([]byte("x-access-token:" + cfg.GitHub.Token)))
	return &Git{
		cfg: cfg, src: src, dry: dry, root: filepath.Join(cfg.Paths.StateDir, "repos"), stall: 15 * time.Second,
		opts: []string{
			"-c", "pack.threads=1", "-c", "pack.windowMemory=64m",
			"-c", "pack.deltaCacheSize=32m", "-c", "core.packedGitLimit=128m",
			"-c", "gc.autoDetach=false",
			"-c", "http.lowSpeedLimit=1000", "-c", "http.lowSpeedTime=60",
		},
	}
}

func (g *Git) Environment() ([]string, error) {
	header, err := g.src.GitHeader()
	if err != nil {
		return nil, err
	}
	logx.Hide(strings.TrimPrefix(header, "Authorization: "))
	basic := base64.StdEncoding.EncodeToString([]byte("x-access-token:" + g.cfg.GitHub.Token))
	ghScope := strings.TrimRight(g.cfg.GitHub.GitURL, "/") + "/"
	return append(os.Environ(),
		"GIT_TERMINAL_PROMPT=0",
		"GIT_CONFIG_GLOBAL="+os.DevNull,
		"GIT_CONFIG_SYSTEM="+os.DevNull,
		"GIT_CONFIG_COUNT=2",
		"GIT_CONFIG_KEY_0=http."+g.cfg.SourceScope()+".extraheader",
		"GIT_CONFIG_VALUE_0="+header,
		"GIT_CONFIG_KEY_1=http."+ghScope+".extraheader",
		"GIT_CONFIG_VALUE_1=Authorization: Basic "+basic,
	), nil
}

func (g *Git) bare(id int64) string { return filepath.Join(g.root, fmt.Sprintf("%d.git", id)) }

var errStalled = errors.New("stalled")

type tail struct {
	mu    sync.Mutex
	last  time.Time
	lines []string
	buf   []byte
}

func (t *tail) Write(b []byte) (int, error) {
	t.mu.Lock()
	defer t.mu.Unlock()
	t.last = time.Now()
	t.buf = append(t.buf, b...)
	for {
		i := bytes.IndexAny(t.buf, "\r\n")
		if i < 0 {
			break
		}
		if line := strings.TrimSpace(string(t.buf[:i])); line != "" {
			t.lines = append(t.lines, line)
			if len(t.lines) > 3 {
				t.lines = t.lines[1:]
			}
		}
		t.buf = t.buf[i+1:]
	}
	return len(b), nil
}

func (t *tail) silentFor() time.Duration {
	t.mu.Lock()
	defer t.mu.Unlock()
	return time.Since(t.last)
}

func (t *tail) text() string {
	t.mu.Lock()
	defer t.mu.Unlock()
	return strings.Join(t.lines, " | ")
}

// A stalled connection writes nothing at all, a big transfer keeps reporting progress.
func (g *Git) run(env []string, dir string, args ...string) (string, error) {
	ctx := context.Background()
	if t := time.Duration(g.cfg.Sync.GitTimeout) * time.Second; t > 0 {
		var cancel context.CancelFunc
		ctx, cancel = context.WithTimeout(ctx, t)
		defer cancel()
	}
	cmd := exec.CommandContext(ctx, "git", append(append([]string{}, g.opts...), args...)...)
	cmd.Dir, cmd.Env, cmd.WaitDelay = dir, env, 5*time.Second
	cmd.SysProcAttr = groupAttr()
	cmd.Cancel = func() error { return killTree(cmd.Process.Pid) }
	var out bytes.Buffer
	errs := &tail{last: time.Now()}
	cmd.Stdout, cmd.Stderr = &out, errs
	if err := cmd.Start(); err != nil {
		if _, ok := err.(*exec.Error); ok {
			return "", fmt.Errorf("git is not installed")
		}
		return "", err
	}
	done := make(chan struct{})
	var stalled atomic.Bool
	go func() {
		tick := time.NewTicker(g.stall / 10)
		defer tick.Stop()
		for {
			select {
			case <-done:
				return
			case <-tick.C:
				if errs.silentFor() > g.stall {
					stalled.Store(true)
					killTree(cmd.Process.Pid)
					return
				}
			}
		}
	}()
	err := cmd.Wait()
	close(done)
	switch {
	case stalled.Load():
		return "", fmt.Errorf("git %s: %w for %s", args[0], errStalled, g.stall)
	case ctx.Err() == context.DeadlineExceeded:
		return "", fmt.Errorf("git %s timed out", args[0])
	case err != nil:
		msg := errs.text()
		if msg == "" {
			msg = strings.TrimSpace(out.String())
		}
		return "", fmt.Errorf("git %s failed: %s", args[0], msg)
	}
	return out.String(), nil
}

func (g *Git) attempt(env []string, dir string, args ...string) (string, error) {
	var out string
	var err error
	for try := 0; try < 3; try++ {
		if out, err = g.run(env, dir, args...); !errors.Is(err, errStalled) {
			return out, err
		}
		logx.Warnf("%v, trying again", err)
	}
	return out, err
}

type Result struct {
	Summary string
	Refs    string
}

// last is the Refs of the previous successful push; when the fetch brings nothing new the
// push is skipped. Pass "" to push regardless.
func (g *Git) Mirror(r *forge.Repo, ghOwner, ghName, last string) (Result, error) {
	if g.dry {
		logx.Infof("dry-run: mirror %s -> %s/%s", r.Full(), ghOwner, ghName)
		return Result{Summary: "dry-run"}, nil
	}
	env, err := g.Environment()
	if err != nil {
		return Result{}, err
	}
	bare := g.bare(r.ID)
	if _, err := os.Stat(bare); err != nil {
		if err := os.MkdirAll(bare, 0o755); err != nil {
			return Result{}, err
		}
		if _, err := g.run(env, "", "init", "--bare", "-q", bare); err != nil {
			return Result{}, err
		}
	}
	dst := fmt.Sprintf("%s/%s/%s.git", strings.TrimRight(g.cfg.GitHub.GitURL, "/"), ghOwner, ghName)
	refs := []string{"+refs/heads/*:refs/heads/*"}
	kinds := []string{"refs/heads"}
	if g.cfg.Sync.Tags {
		refs = append(refs, "+refs/tags/*:refs/tags/*")
		kinds = append(kinds, "refs/tags")
	}
	start := time.Now()
	fetch := append([]string{"fetch", "--progress", "--prune", "--update-head-ok", g.src.CloneURL(r)}, refs...)
	if _, err := g.attempt(env, bare, fetch...); err != nil {
		return Result{}, err
	}
	fetched := time.Since(start)
	listing, err := g.run(env, bare, append([]string{"for-each-ref", "--format=%(objectname) %(refname)"}, kinds...)...)
	if err != nil {
		return Result{}, err
	}
	sum := sha1.Sum([]byte(listing))
	state := hex.EncodeToString(sum[:])
	if last != "" && last == state {
		logx.Debugf("%s: nothing new since the last push", r.Full())
		return Result{Summary: "up to date", Refs: state}, nil
	}
	out, err := g.attempt(env, bare, append([]string{"push", "--progress", "--porcelain", "--prune", dst}, refs...)...)
	if err != nil {
		return Result{}, err
	}
	logx.Debugf("%s: fetch took %s, push took %s", r.Full(), fetched.Round(time.Millisecond), (time.Since(start) - fetched).Round(time.Millisecond))
	changed, rejected := 0, 0
	for _, line := range strings.Split(out, "\n") {
		switch {
		case line == "":
		case strings.ContainsRune("*+- ", rune(line[0])):
			changed++
		case line[0] == '!':
			rejected++
		}
	}
	if rejected > 0 {
		return Result{}, fmt.Errorf("GitHub rejected %d ref(s)", rejected)
	}
	if changed == 0 {
		return Result{Summary: "up to date", Refs: state}, nil
	}
	return Result{Summary: fmt.Sprintf("%d ref(s) updated", changed), Refs: state}, nil
}

func (g *Git) Forget(id int64) { os.RemoveAll(g.bare(id)) }
