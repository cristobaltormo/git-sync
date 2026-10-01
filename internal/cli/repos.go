package cli

import (
	"encoding/json"
	"flag"
	"fmt"
	"os"
	"strconv"
	"strings"
	"time"

	"github.com/cristobaltormo/git-sync/internal/config"
)

const reposUsage = `Usage: gitsync repos [command]

With no command in a terminal, opens the interactive selector.

  list [--status S] [--json]     show every repository and what gitsync does with it
  allow <repo>... | --pending    sync these repositories
  ignore <repo>...               never sync these repositories
  reset <repo>...                forget the decision, the general rules apply again
  set <repo> key=value ...       per-repository options: name, keep_private, tags,
                                 metadata, on_delete, note (key= clears one)

A repo is owner/name, just the name, or a pattern such as 'my-org/*'.
`

func cmdRepos(cfgPath string, args []string) (int, error) {
	sub := ""
	rest := args
	if len(args) > 0 && !strings.HasPrefix(args[0], "-") {
		sub, rest = args[0], args[1:]
	}
	if sub == "help" {
		fmt.Print(reposUsage)
		return 0, nil
	}
	if sub == "" && isTerminal() && colorTTY() {
		return cmdReposTUI(cfgPath, rest)
	}
	switch sub {
	case "", "list", "ls":
		return reposList(cfgPath, rest)
	case "allow", "ignore", "reset":
		return reposDecide(cfgPath, sub, rest)
	case "set":
		return reposSet(cfgPath, rest)
	}
	return 2, config.Errorf("unknown command %q\n\n%s", sub, reposUsage)
}

func colorTTY() bool {
	st, err := os.Stdout.Stat()
	return err == nil && st.Mode()&os.ModeCharDevice != 0
}

func openApp(cfgPath string, args []string, setup func(*flag.FlagSet)) (*app, *flag.FlagSet, error) {
	path, fs, err := flags("repos", cfgPath, args, setup)
	if err != nil {
		return nil, nil, err
	}
	cfg, err := load(path)
	if err != nil {
		return nil, nil, err
	}
	a, err := build(cfg)
	if err != nil {
		return nil, nil, err
	}
	return a, fs, nil
}

func statusStyle(st string) style {
	switch st {
	case stSynced:
		return green
	case stError:
		return red
	case stWaiting, stQueued:
		return yellow
	}
	return dim
}

func note(it *item) string {
	var parts []string
	switch it.Status {
	case stWaiting:
		parts = append(parts, "new: gitsync repos allow "+it.full())
	case stIgnored, stFiltered:
		parts = append(parts, it.Verdict.Why)
		if it.managed() {
			parts = append(parts, "GitHub copy left as it is")
		}
	case stError:
		parts = append(parts, clip(it.Entry.LastError, 60))
	case stQueued:
		parts = append(parts, "will be created on GitHub")
	}
	if it.Verdict.Rule != nil {
		if it.Verdict.Rule.Name != "" {
			parts = append(parts, "as "+it.Verdict.Rule.Name)
		}
		if it.Verdict.Rule.KeepPrivate != nil && *it.Verdict.Rule.KeepPrivate {
			parts = append(parts, "kept private")
		}
	}
	return strings.Join(parts, "; ")
}

func lastSync(it *item) string {
	if it.Has && it.Entry.LastOK > 0 {
		return ago(time.Now().Unix() - it.Entry.LastOK)
	}
	return "-"
}

func reposList(cfgPath string, args []string) (int, error) {
	var status *string
	var asJSON *bool
	a, _, err := openApp(cfgPath, args, func(fs *flag.FlagSet) {
		status = fs.String("status", "", "only show: synced, waiting, ignored, filtered, error, queued")
		asJSON = fs.Bool("json", false, "machine-readable output")
	})
	if err != nil {
		return 2, err
	}
	items, err := inventory(a)
	if err != nil {
		return 1, err
	}
	shown := items[:0:0]
	for _, it := range items {
		if *status == "" || it.Status == *status {
			shown = append(shown, it)
		}
	}
	if *asJSON {
		type row struct {
			Repo    string `json:"repo"`
			Status  string `json:"status"`
			GitHub  string `json:"github,omitempty"`
			Reason  string `json:"reason,omitempty"`
			Role    string `json:"role,omitempty"`
			Private bool   `json:"private"`
			Error   string `json:"error,omitempty"`
		}
		out := make([]row, 0, len(shown))
		for _, it := range shown {
			out = append(out, row{it.full(), it.Status, it.target(), it.Verdict.Why, it.Repo.Role,
				it.Repo.Private, it.Entry.LastError})
		}
		enc := json.NewEncoder(os.Stdout)
		enc.SetIndent("", "  ")
		return 0, enc.Encode(out)
	}
	c := counts(items)
	fmt.Printf("%s  %s -> GitHub\n", paint("gitsync", bold), a.cfg.SourceURL())
	fmt.Printf(" %d repositories: %d synced, %d waiting for you, %d ignored, %d filtered out",
		len(items), c[stSynced]+c[stError]+c[stQueued], c[stWaiting], c[stIgnored], c[stFiltered])
	if c[stError] > 0 {
		fmt.Printf(", %s", paint(strconv.Itoa(c[stError])+" with errors", red))
	}
	fmt.Print("\n\n")
	t := &table{head: []string{"STATUS", "REPOSITORY", "GITHUB", "LAST SYNC", "NOTES"}}
	for _, it := range shown {
		dst := it.target()
		if dst == "" {
			dst = "-"
		}
		t.add([]string{it.Status, it.full(), dst, lastSync(it), note(it)}, statusStyle(it.Status))
	}
	t.print()
	if c[stWaiting] > 0 && *status == "" {
		fmt.Printf("\n%s\n", paint(fmt.Sprintf(" %d new repositor%s waiting. Sync them with `gitsync repos allow <repo>` or `--pending`, skip them with `gitsync repos ignore <repo>`.",
			c[stWaiting], map[bool]string{true: "y is", false: "ies are"}[c[stWaiting] == 1]), yellow))
	}
	return 0, nil
}

func saveRepos(a *app, change func(r *config.Repos) error) error {
	r := a.cfg.Repos
	if r == nil {
		var err error
		if r, err = config.LoadRepos(config.ReposPath(a.cfg)); err != nil {
			return err
		}
	}
	if err := change(r); err != nil {
		return err
	}
	if err := r.Save(); err != nil {
		if os.IsPermission(err) || strings.Contains(err.Error(), "permission denied") {
			return config.Errorf("cannot write %s: permission denied (run as the user that owns the config, e.g. with sudo)", r.Path)
		}
		return err
	}
	return nil
}

func applied(a *app) {
	fmt.Printf("Saved to %s.\n", config.ReposPath(a.cfg))
	if a.cfg.Sync.PollInterval == 0 {
		fmt.Println("A running gitsync reloads on `systemctl reload gitsync` (polling is off).")
		return
	}
	fmt.Printf("A running gitsync picks it up within %d seconds.\n", max(a.cfg.Sync.PollInterval, 1))
}

func reposDecide(cfgPath, verb string, args []string) (int, error) {
	var pending *bool
	a, fs, err := openApp(cfgPath, args, func(fs *flag.FlagSet) {
		pending = fs.Bool("pending", false, "every repository that is waiting for a decision")
	})
	if err != nil {
		return 2, err
	}
	items, err := inventory(a)
	if err != nil {
		return 1, err
	}
	var chosen []*item
	if *pending {
		for _, it := range items {
			if it.Status == stWaiting {
				chosen = append(chosen, it)
			}
		}
	}
	if fs.NArg() > 0 {
		more, err := resolve(items, fs.Args())
		if err != nil {
			return 2, err
		}
		chosen = append(chosen, more...)
	}
	if len(chosen) == 0 {
		if *pending {
			fmt.Println("Nothing is waiting for a decision.")
			return 0, nil
		}
		return 2, config.Errorf("name at least one repository (see `gitsync repos help`)")
	}
	err = saveRepos(a, func(r *config.Repos) error {
		for _, it := range chosen {
			if err := r.Edit(it.full(), func(x *config.RepoRule) {
				switch verb {
				case "allow":
					t := true
					x.Sync = &t
				case "ignore":
					f := false
					x.Sync = &f
				default:
					x.Sync = nil
				}
			}); err != nil {
				return err
			}
		}
		return nil
	})
	if err != nil {
		return 1, err
	}
	past := map[string]string{"allow": "will be synced", "ignore": "will be ignored", "reset": "follows the general rules again"}[verb]
	for _, it := range chosen {
		fmt.Printf("  %s %s\n", pad(it.full(), 40), past)
	}
	applied(a)
	if verb == "ignore" {
		for _, it := range chosen {
			if it.managed() {
				fmt.Printf("Note: %s keeps its GitHub copy; gitsync stops updating it and never deletes it for this.\n", it.full())
			}
		}
	}
	return 0, nil
}

func parseBoolOpt(v string) (*bool, error) {
	switch strings.ToLower(v) {
	case "", "default":
		return nil, nil
	case "true", "yes", "on", "1":
		t := true
		return &t, nil
	case "false", "no", "off", "0":
		f := false
		return &f, nil
	}
	return nil, config.Errorf("%q is not true or false", v)
}

func applyOption(x *config.RepoRule, key, val string) error {
	var err error
	switch key {
	case "name":
		x.Name = val
	case "on_delete":
		x.OnDelete = val
	case "note":
		x.Note = val
	case "keep_private":
		x.KeepPrivate, err = parseBoolOpt(val)
	case "tags":
		x.Tags, err = parseBoolOpt(val)
	case "metadata":
		x.Metadata, err = parseBoolOpt(val)
	default:
		return config.Errorf("unknown option %q (name, keep_private, tags, metadata, on_delete, note)", key)
	}
	return err
}

func reposSet(cfgPath string, args []string) (int, error) {
	a, fs, err := openApp(cfgPath, args, nil)
	if err != nil {
		return 2, err
	}
	if fs.NArg() < 2 {
		return 2, config.Errorf("usage: gitsync repos set <repo> key=value ...")
	}
	items, err := inventory(a)
	if err != nil {
		return 1, err
	}
	chosen, err := resolve(items, fs.Args()[:1])
	if err != nil {
		return 2, err
	}
	if len(chosen) != 1 {
		return 2, config.Errorf("%q matches %d repositories; set options on one at a time", fs.Arg(0), len(chosen))
	}
	opts := map[string]string{}
	for _, kv := range fs.Args()[1:] {
		k, v, ok := strings.Cut(kv, "=")
		if !ok {
			return 2, config.Errorf("%q is not key=value", kv)
		}
		opts[strings.TrimSpace(k)] = strings.TrimSpace(v)
	}
	it := chosen[0]
	err = saveRepos(a, func(r *config.Repos) error {
		var optErr error
		if err := r.Edit(it.full(), func(x *config.RepoRule) {
			for k, v := range opts {
				if e := applyOption(x, k, v); e != nil && optErr == nil {
					optErr = e
				}
			}
		}); err != nil {
			return err
		}
		return optErr
	})
	if err != nil {
		return 2, err
	}
	fmt.Printf("%s updated.\n", it.full())
	applied(a)
	return 0, nil
}
