package cli

import (
	"fmt"
	"os"
	"os/exec"
	"runtime"
	"sort"
	"strings"

	"github.com/cristobaltormo/git-sync/internal/config"
	"github.com/cristobaltormo/git-sync/internal/engine"
)

const configUsage = `Usage: gitsync config <command>

  show [--all]                  current settings (--all includes the ones left at their default)
  get <key>                     print one setting, e.g. sync.poll_interval
  set <key> <value>...          change a setting; comments in the file are kept and the
                                result is validated before anything is written
  profile [name]                list the ready-made profiles, or apply one
  edit                          open the file in your editor and validate it afterwards
  path                          print the config file location
  test-notify                   send a test message to notify.url
`

func cmdConfig(cfgPath string, args []string) (int, error) {
	sub, rest := "show", args
	if len(args) > 0 && !strings.HasPrefix(args[0], "-") {
		sub, rest = args[0], args[1:]
	}
	path, fs, err := flags("config", cfgPath, rest, nil)
	if err != nil {
		return 2, err
	}
	file := config.Find(path)
	switch sub {
	case "help":
		fmt.Print(configUsage)
		return 0, nil
	case "path":
		fmt.Println(file)
		return 0, nil
	case "edit":
		return configEdit(file)
	}
	cfg, err := config.Load(file)
	if err != nil {
		return 2, err
	}
	switch sub {
	case "show", "list":
		all := false
		for _, a := range fs.Args() {
			all = all || a == "--all" || a == "-all"
		}
		configShow(cfg, all)
		return 0, nil
	case "get":
		if fs.NArg() != 1 {
			return 2, config.Errorf("usage: gitsync config get <key>")
		}
		k, ok := config.FindKey(fs.Arg(0))
		if !ok {
			return 2, unknownKey(fs.Arg(0))
		}
		fmt.Println(shown(k, cfg))
		return 0, nil
	case "set":
		return configSet(file, cfg, fs.Args())
	case "profile":
		return configProfile(file, fs.Args())
	case "test-notify":
		if cfg.Notify.URL == "" {
			return 2, config.Errorf("notify.url is not set")
		}
		if err := engine.SendTest(cfg.Notify.URL, cfg.Notify.Format); err != nil {
			return 1, err
		}
		fmt.Println("Test message sent.")
		return 0, nil
	}
	return 2, config.Errorf("unknown command %q\n\n%s", sub, configUsage)
}

func unknownKey(name string) error {
	var near []string
	for _, k := range config.Keys() {
		if strings.Contains(k.Name, strings.ToLower(name)) {
			near = append(near, k.Name)
		}
	}
	if len(near) > 0 && len(near) < 8 {
		return config.Errorf("no setting called %q; did you mean %s?", name, strings.Join(near, ", "))
	}
	return config.Errorf("no setting called %q (see `gitsync config show --all`)", name)
}

func shown(k config.Key, c *config.Config) string {
	if k.Secret {
		if k.Get(c) == "" {
			return "(not set)"
		}
		return "(set, hidden)"
	}
	return k.Get(c)
}

func configShow(cfg *config.Config, all bool) {
	def := config.Default()
	section := ""
	for _, k := range config.Keys() {
		v := k.Get(cfg)
		isDef := v == k.Get(def)
		if !all && isDef && !k.Secret {
			continue
		}
		if sec, _, _ := strings.Cut(k.Name, "."); sec != section {
			if section != "" {
				fmt.Println()
			}
			section = sec
			fmt.Println(paint("["+sec+"]", bold))
		}
		_, name, _ := strings.Cut(k.Name, ".")
		val := shown(k, cfg)
		if val == "" {
			val = paint("(empty)", dim)
		}
		line := fmt.Sprintf("  %-18s %s", name, val)
		if isDef && !k.Secret {
			line = paint(line+"   default", dim)
		}
		fmt.Println(line)
	}
	if len(cfg.Accounts) > 0 {
		fmt.Println()
		fmt.Println(paint("[accounts]", bold))
		names := make([]string, 0, len(cfg.Accounts))
		w := 0
		for s := range cfg.Accounts {
			names = append(names, s)
			w = max(w, len(s))
		}
		sort.Strings(names)
		for _, s := range names {
			fmt.Printf("  %s = %s\n", pad(s, w), cfg.Accounts[s])
		}
	}
}

func configSet(file string, cfg *config.Config, args []string) (int, error) {
	if len(args) < 2 {
		return 2, config.Errorf("usage: gitsync config set <key> <value>...")
	}
	k, ok := config.FindKey(args[0])
	if !ok {
		return 2, unknownKey(args[0])
	}
	if k.Secret {
		return 2, config.Errorf("%s is a secret: put it in the file by hand, or use token_file / the environment, so it does not end up in your shell history", k.Name)
	}
	lit, err := k.Literal(args[1:])
	if err != nil {
		return 2, err
	}
	if err := config.SetValues(file, map[string]string{k.Name: lit}); err != nil {
		return 2, err
	}
	fmt.Printf("%s = %s\n", k.Name, lit)
	fmt.Println(reloadHint(cfg))
	return 0, nil
}

func reloadHint(cfg *config.Config) string {
	if cfg.Sync.PollInterval == 0 {
		return "Saved. A running gitsync applies it on `systemctl reload gitsync` (polling is off)."
	}
	return fmt.Sprintf("Saved. A running gitsync applies it within %d seconds (listen.* needs a restart).", max(cfg.Sync.PollInterval, 1))
}

func configProfile(file string, args []string) (int, error) {
	if len(args) == 0 {
		for _, p := range config.Profiles {
			fmt.Println(" " + p.Describe())
		}
		fmt.Println("\nApply one with `gitsync config profile <name>`. It only changes the handful of settings it names.")
		return 0, nil
	}
	p, ok := config.FindProfile(args[0])
	if !ok {
		return 2, config.Errorf("no profile called %q (personal, team, careful)", args[0])
	}
	if err := config.SetValues(file, p.Set); err != nil {
		return 2, err
	}
	fmt.Printf("Profile %s applied:\n", p.Name)
	for _, k := range sortedKeys(p.Set) {
		fmt.Printf("  %-20s = %s\n", k, p.Set[k])
	}
	cfg, err := config.Load(file)
	if err == nil {
		fmt.Println(reloadHint(cfg))
	}
	return 0, nil
}

func configEdit(file string) (int, error) {
	editor := os.Getenv("VISUAL")
	if editor == "" {
		editor = os.Getenv("EDITOR")
	}
	if editor == "" {
		for _, c := range []string{"nano", "vim", "vi"} {
			if _, err := exec.LookPath(c); err == nil {
				editor = c
				break
			}
		}
		if runtime.GOOS == "windows" {
			editor = "notepad"
		}
	}
	if editor == "" {
		return 2, config.Errorf("no editor found: set $EDITOR")
	}
	parts := strings.Fields(editor)
	cmd := exec.Command(parts[0], append(parts[1:], file)...)
	cmd.Stdin, cmd.Stdout, cmd.Stderr = os.Stdin, os.Stdout, os.Stderr
	if err := cmd.Run(); err != nil {
		return 1, err
	}
	if _, err := config.Load(file); err != nil {
		return 2, config.Errorf("saved, but the file has a problem: %v", err)
	}
	fmt.Println("The file is valid.")
	return 0, nil
}

func sortedKeys(m map[string]string) []string {
	out := make([]string, 0, len(m))
	for k := range m {
		out = append(out, k)
	}
	sort.Strings(out)
	return out
}
