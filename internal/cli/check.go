package cli

import (
	"fmt"
	"os/exec"
	"sort"
	"strings"

	"github.com/cristobaltormo/git-sync/internal/github"
)

func mark(state, format string, a ...any) {
	fmt.Printf("  %-4s %s\n", state, fmt.Sprintf(format, a...))
}

func cmdCheck(cfgPath string, args []string) (int, error) {
	path, _, err := flags("check", cfgPath, args, nil)
	if err != nil {
		return 2, err
	}
	cfg, err := load(path)
	if err != nil {
		return 2, err
	}
	fmt.Printf("config %s: ok\n", cfg.Path)
	a, err := build(cfg)
	if err != nil {
		return 2, err
	}
	gh := github.New(cfg, false)
	problems := 0

	me, err := a.src.Whoami()
	if err != nil {
		mark("FAIL", "source: %v", err)
		return 1, nil
	}
	admin := ""
	if me.Admin {
		admin = " (admin)"
	}
	mark("ok", "source token works, logged in as %s%s", me.Login, admin)
	if !me.SystemHooks && cfg.Hooks.Mode == "auto" && a.src.Hooks() != nil {
		mark("info", "webhooks will be created per repo")
	}

	login, scopes, err := gh.Whoami()
	if err != nil {
		mark("FAIL", "GitHub: %v", err)
		return 1, nil
	}
	mark("ok", "GitHub token works, logged in as %s", login)
	if scopes != nil {
		have := map[string]bool{}
		for _, s := range strings.Split(*scopes, ",") {
			have[strings.TrimSpace(s)] = true
		}
		if !have["repo"] {
			mark("FAIL", "GitHub token has no 'repo' scope")
			problems++
		}
		if cfg.Sync.OnDelete == "delete" && !have["delete_repo"] {
			mark("warn", "no 'delete_repo' scope: deleting mirrors will fail (or set on_delete = \"archive\")")
		}
	} else {
		mark("info", "fine-grained token: it needs Administration and Contents write on the target repos")
	}

	owners := make([]string, 0, len(cfg.Accounts))
	for o := range cfg.Accounts {
		owners = append(owners, o)
	}
	sort.Strings(owners)
	for _, src := range owners {
		dst := cfg.Accounts[src]
		if repos, err := a.src.List(src); err != nil {
			mark("FAIL", "source account '%s': %v", src, err)
			problems++
		} else {
			mark("ok", "source account '%s': %d repo(s) visible", src, len(repos))
		}
		kind, err := gh.OwnerType(dst)
		switch {
		case err != nil:
			mark("FAIL", "GitHub account '%s': %v", dst, err)
			problems++
		case kind == "User" && !strings.EqualFold(dst, login):
			mark("FAIL", "GitHub account '%s' is another user; the token can only create repos under '%s' or an organization", dst, login)
			problems++
		default:
			mark("ok", "GitHub account '%s' (%s)", dst, strings.ToLower(kind))
		}
	}

	if out, err := exec.Command("git", "--version").Output(); err != nil {
		mark("FAIL", "git is not installed")
		problems++
	} else {
		mark("ok", "%s", strings.TrimSpace(string(out)))
	}
	h := cfg.Listen.Host
	if h != "127.0.0.1" && h != "localhost" && h != "::1" && cfg.Listen.PublicURL == "" {
		mark("warn", "listening beyond localhost without listen.public_url: the webhooks will point at 127.0.0.1")
	}
	mark("info", "webhook URL: %s", cfg.HookURL())
	if problems == 0 {
		fmt.Println("all good")
		return 0, nil
	}
	fmt.Printf("%d problem(s)\n", problems)
	return 1, nil
}
