package cli

import (
	"encoding/json"
	"flag"
	"fmt"
	"os"
	"sort"
	"strings"

	"github.com/cristobaltormo/git-sync/internal/config"
)

type planRow struct {
	Repo   string `json:"repo"`
	Action string `json:"action"`
	Detail string `json:"detail,omitempty"`
}

func cmdPlan(cfgPath string, args []string) (int, error) {
	var asJSON *bool
	a, _, err := openApp(cfgPath, args, func(fs *flag.FlagSet) {
		asJSON = fs.Bool("json", false, "machine-readable output")
	})
	if err != nil {
		return 2, err
	}
	items, err := inventory(a)
	if err != nil {
		return 1, err
	}
	var rows []planRow
	seen := map[int64]bool{}
	for _, it := range items {
		seen[it.Repo.ID] = true
		row, err := planItem(a, it)
		if err != nil {
			return 1, err
		}
		rows = append(rows, row)
	}
	for id, x := range a.eng.Entries() {
		if seen[id] || !x.Managed {
			continue
		}
		od := a.cfg.Sync.OnDelete
		if r := a.cfg.Repos.Rule(x.Owner + "/" + x.Name); r != nil && r.OnDelete != "" {
			od = r.OnDelete
		}
		act := map[string]string{"delete": "delete the GitHub copy", "archive": "archive the GitHub copy", "ignore": "leave the GitHub copy"}[od]
		rows = append(rows, planRow{x.Owner + "/" + x.Name, "removed", "gone from the source: " + act})
	}
	sort.SliceStable(rows, func(i, j int) bool { return order(rows[i].Action) < order(rows[j].Action) })
	if *asJSON {
		enc := json.NewEncoder(os.Stdout)
		enc.SetIndent("", "  ")
		return 0, enc.Encode(rows)
	}
	tally := map[string]int{}
	t := &table{head: []string{"ACTION", "REPOSITORY", "DETAIL"}}
	for _, r := range rows {
		tally[r.Action]++
		if r.Action == "up to date" || r.Action == "skip" {
			continue
		}
		t.add([]string{r.Action, r.Repo, r.Detail}, planStyle(r.Action))
	}
	if len(t.rows) > 0 {
		t.print()
		fmt.Println()
	}
	var parts []string
	for _, k := range []string{"create", "update", "rename", "blocked", "removed", "waiting", "up to date", "skip"} {
		if tally[k] > 0 {
			parts = append(parts, fmt.Sprintf("%d %s", tally[k], k))
		}
	}
	fmt.Println(" " + strings.Join(parts, ", "))
	fmt.Println(paint(" Nothing was changed. `gitsync run` or `gitsync sync` applies it.", dim))
	return 0, nil
}

func order(a string) int {
	for i, k := range []string{"blocked", "create", "rename", "update", "removed", "waiting", "up to date", "skip"} {
		if k == a {
			return i
		}
	}
	return 99
}

func planStyle(a string) style {
	switch a {
	case "create", "update", "rename":
		return green
	case "blocked", "removed":
		return red
	case "waiting":
		return yellow
	}
	return dim
}

func planItem(a *app, it *item) (planRow, error) {
	r := it.Repo
	row := planRow{Repo: it.full()}
	switch it.Status {
	case stWaiting:
		row.Action, row.Detail = "waiting", "new repository, needs your decision"
		return row, nil
	case stIgnored, stFiltered:
		row.Action, row.Detail = "skip", it.Verdict.Why
		if it.managed() {
			row.Detail += " (GitHub copy stays)"
		}
		return row, nil
	}
	owner, ok := a.cfg.GitHubOwner(r.Owner)
	if !ok {
		row.Action, row.Detail = "skip", "no GitHub account mapped for "+r.Owner
		return row, nil
	}
	name := r.Name
	if it.Verdict.Rule != nil && it.Verdict.Rule.Name != "" {
		name = it.Verdict.Rule.Name
	}
	target := owner + "/" + name
	if !it.managed() {
		g, err := a.gh.Get(owner, name)
		if err != nil {
			return row, config.Errorf("asking GitHub about %s: %v", target, err)
		}
		switch {
		case g == nil:
			row.Action, row.Detail = "create", "new private repository "+target+", then make it public if the source is"
			if r.Private || !a.cfg.Sync.Visibility {
				row.Detail = "new private repository " + target
			}
		case a.cfg.Sync.AdoptExisting:
			row.Action, row.Detail = "update", target+" already exists: adopted, overwritten to match"
		default:
			row.Action, row.Detail = "blocked", target+" already exists on GitHub and was not created by gitsync (sync.adopt_existing)"
		}
		return row, nil
	}
	switch {
	case it.Entry.GHName != name || !strings.EqualFold(it.Entry.GHOwner, owner):
		row.Action, row.Detail = "rename", it.Entry.GHOwner+"/"+it.Entry.GHName+" -> "+target
	case it.Entry.LastError != "":
		row.Action, row.Detail = "update", "retry after: "+clip(it.Entry.LastError, 70)
	case it.Entry.Sig != r.Sig() || it.Entry.Updated != r.Updated:
		row.Action, row.Detail = "update", "changed on the source"
	default:
		row.Action = "up to date"
	}
	return row, nil
}
