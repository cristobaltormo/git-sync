package cli

import (
	"flag"
	"fmt"

	"github.com/cristobaltormo/git-sync/internal/config"
)

func cmdHooks(cfgPath string, args []string) (int, error) {
	var force, remove *bool
	path, _, err := flags("hooks", cfgPath, args, func(fs *flag.FlagSet) {
		force = fs.Bool("force", false, "rewrite existing webhooks (after changing the secret)")
		remove = fs.Bool("remove", false, "delete the webhooks gitsync created")
	})
	if err != nil {
		return 2, err
	}
	cfg, err := load(path)
	if err != nil {
		return 2, err
	}
	a, err := build(cfg)
	if err != nil {
		return 2, err
	}
	hooks := a.src.Hooks()
	if hooks == nil {
		return 2, config.Errorf("%s has no webhook management here: create the webhook by hand, pointing at %s", cfg.Source.Type, cfg.HookURL())
	}
	if *remove {
		n := 0
		if hooks.RemoveGlobal(a.eng.SystemHook()) {
			n++
			a.eng.SetSystemHook(0)
		}
		repos, err := a.eng.ListAll()
		if err != nil {
			return 1, err
		}
		for _, r := range repos {
			n += hooks.RemoveRepo(r)
		}
		fmt.Printf("removed %d webhook(s)\n", n)
		return 0, nil
	}
	a.eng.SetupHooks(*force)
	if a.eng.HookMode() == "repo" {
		repos, err := a.eng.ListAll()
		if err != nil {
			return 1, err
		}
		for _, r := range repos {
			if ok, _ := a.eng.Selected(r); !ok {
				continue
			}
			res, err := hooks.Repo(r, *force)
			if err != nil {
				res = err.Error()
			}
			fmt.Printf("%s: %s\n", r.Full(), res)
		}
	}
	return 0, nil
}
