package cli

import (
	"flag"
	"fmt"
	"os"
	"strconv"
	"strings"
	"time"

	"github.com/cristobaltormo/git-sync/internal/config"
	"github.com/cristobaltormo/git-sync/internal/engine"
)

func cmdSync(cfgPath string, args []string) (int, error) {
	var dry *bool
	path, fs, err := flags("sync", cfgPath, args, func(fs *flag.FlagSet) {
		dry = fs.Bool("dry-run", false, "log what would happen, change nothing")
	})
	if err != nil {
		return 2, err
	}
	cfg, err := load(path)
	if err != nil {
		return 2, err
	}
	if *dry {
		cfg.Sync.DryRun = true
	}
	a, err := build(cfg)
	if err != nil {
		return 2, err
	}
	if err := os.MkdirAll(cfg.Paths.StateDir, 0o755); err != nil {
		return 2, config.Errorf("cannot create %s: %v", cfg.Paths.StateDir, err)
	}
	a.eng.Start()
	wanted := map[string]bool{}
	for _, r := range fs.Args() {
		wanted[strings.ToLower(r)] = true
	}
	repos, err := a.eng.ListAll()
	if err != nil {
		return 1, err
	}
	n := 0
	for _, r := range repos {
		if len(wanted) > 0 && !wanted[strings.ToLower(r.Full())] && !wanted[strings.ToLower(r.Name)] {
			continue
		}
		if ok, _ := a.eng.Selected(r); ok {
			a.eng.Enqueue(&engine.Job{Key: strconv.FormatInt(r.ID, 10), Repo: r, Why: "sync", Verify: true, Force: true})
			n++
		}
	}
	if len(wanted) > 0 && n == 0 {
		return 2, config.Errorf("no repo matched %s", strings.Join(fs.Args(), ", "))
	}
	a.eng.Drain(0)
	if len(wanted) == 0 {
		a.eng.Reconcile(false, "sync")
		a.eng.Drain(0)
		if a.eng.HasMissing() {
			time.Sleep(time.Duration(cfg.Sync.DeleteGrace+1) * time.Second)
			a.eng.Reconcile(false, "sync")
			a.eng.Drain(0)
		}
	}
	a.eng.Shutdown()
	failed := 0
	for _, x := range a.eng.Entries() {
		if x.LastError != "" {
			failed++
			fmt.Fprintf(os.Stderr, "failed: %s/%s: %s\n", x.Owner, x.Name, x.LastError)
		}
	}
	fmt.Printf("%d repo(s) processed, %d failed\n", n, failed)
	if failed > 0 {
		return 1, nil
	}
	return 0, nil
}
