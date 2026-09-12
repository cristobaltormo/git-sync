package cli

import (
	"flag"
	"fmt"
	"os"
	"runtime"
	"runtime/debug"

	"github.com/cristobaltormo/git-sync/internal/config"
	"github.com/cristobaltormo/git-sync/internal/engine"
	"github.com/cristobaltormo/git-sync/internal/forge"
	"github.com/cristobaltormo/git-sync/internal/github"
	"github.com/cristobaltormo/git-sync/internal/logx"
	"github.com/cristobaltormo/git-sync/internal/mirror"
)

var Version = "dev"

const usage = `gitsync %s: mirror repos from a self-hosted git server to GitHub, live.

Usage: gitsync [-c config.toml] <command>

  check      test the config, tokens and permissions
  run        run the daemon (--dry-run to only log what would happen)
  sync       mirror everything once and exit (owner/name ... for just some)
  status     show what has been synced
  version    print the version

The config is looked for in ./config.toml and /etc/gitsync/config.toml.
`

type command func(cfgPath string, args []string) (int, error)

var commands = map[string]command{
	"run": cmdRun, "sync": cmdSync, "check": cmdCheck, "status": cmdStatus,
}

func Run(version string, args []string) int {
	Version = version
	debug.SetGCPercent(25)
	runtime.GOMAXPROCS(min(runtime.NumCPU(), 2))

	global := flag.NewFlagSet("gitsync", flag.ContinueOnError)
	global.Usage = func() { fmt.Fprintf(os.Stderr, usage, version) }
	cfgPath := global.String("c", "", "config file")
	global.StringVar(cfgPath, "config", "", "config file")
	if err := global.Parse(args); err != nil {
		return 2
	}
	rest := global.Args()
	if len(rest) == 0 {
		global.Usage()
		return 2
	}
	name := rest[0]
	switch name {
	case "version", "-v", "--version":
		fmt.Println("gitsync", version)
		return 0
	case "help", "-h", "--help":
		fmt.Printf(usage, version)
		return 0
	}
	cmd, ok := commands[name]
	if !ok {
		fmt.Fprintf(os.Stderr, "unknown command %q\n\n", name)
		fmt.Fprintf(os.Stderr, usage, version)
		return 2
	}
	code, err := cmd(*cfgPath, rest[1:])
	if err != nil {
		fmt.Fprintf(os.Stderr, "error: %s\n", logx.Clean(err.Error()))
		if config.IsError(err) {
			return 2
		}
		return 1
	}
	return code
}

func flags(name, cfgPath string, args []string, setup func(*flag.FlagSet)) (string, *flag.FlagSet, error) {
	fs := flag.NewFlagSet(name, flag.ContinueOnError)
	c := fs.String("c", cfgPath, "config file")
	fs.StringVar(c, "config", cfgPath, "config file")
	if setup != nil {
		setup(fs)
	}
	if err := fs.Parse(args); err != nil {
		return "", nil, config.Errorf("%v", err)
	}
	return *c, fs, nil
}

type app struct {
	cfg *config.Config
	src forge.Provider
	gh  *github.Client
	eng *engine.Engine
}

func load(cfgPath string) (*config.Config, error) {
	return config.Load(config.Find(cfgPath))
}

func build(cfg *config.Config) (*app, error) {
	src, err := forge.New(cfg)
	if err != nil {
		return nil, config.Errorf("%v", err)
	}
	gh := github.New(cfg, cfg.Sync.DryRun)
	eng, err := engine.New(cfg, src, gh, mirror.New(cfg, src, cfg.Sync.DryRun))
	if err != nil {
		return nil, err
	}
	return &app{cfg: cfg, src: src, gh: gh, eng: eng}, nil
}
