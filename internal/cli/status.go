package cli

import (
	"fmt"
	"sort"
	"time"

	"github.com/cristobaltormo/git-sync/internal/engine"
)

func cmdStatus(cfgPath string, args []string) (int, error) {
	path, _, err := flags("status", cfgPath, args, nil)
	if err != nil {
		return 2, err
	}
	cfg, err := load(path)
	if err != nil {
		return 2, err
	}
	st, err := engine.ReadState(cfg.Paths.StateDir)
	if err != nil {
		fmt.Printf("no state yet in %s\n", cfg.Paths.StateDir)
		return 0, nil
	}
	rows := make([]*engine.Entry, 0, len(st.Repos))
	width := 4
	for _, x := range st.Repos {
		rows = append(rows, x)
		width = max(width, len(x.Owner+"/"+x.Name))
	}
	sort.Slice(rows, func(i, j int) bool {
		return rows[i].Owner+"/"+rows[i].Name < rows[j].Owner+"/"+rows[j].Name
	})
	now := time.Now().Unix()
	for _, x := range rows {
		vis, dst, when := "-", "-", "never"
		if x.GH != nil {
			vis = "public"
			if x.GH.Private {
				vis = "private"
			}
		}
		if x.GHOwner != "" {
			dst = x.GHOwner + "/" + x.GHName
		}
		if x.LastOK > 0 {
			when = ago(now - x.LastOK)
		}
		line := fmt.Sprintf("%-*s  ->  %-28s %-8s %s", width, x.Owner+"/"+x.Name, dst, vis, when)
		if x.Excluded {
			line += "  (excluded by filter)"
		}
		if x.LastError != "" {
			line += "  ERROR: " + x.LastError
		}
		fmt.Println(line)
	}
	fmt.Printf("%d repo(s)\n", len(rows))
	return 0, nil
}

func ago(sec int64) string {
	switch {
	case sec < 60:
		return "just now"
	case sec < 3600:
		return fmt.Sprintf("%d min ago", sec/60)
	case sec < 86400:
		return fmt.Sprintf("%d h ago", sec/3600)
	}
	return fmt.Sprintf("%d days ago", sec/86400)
}
