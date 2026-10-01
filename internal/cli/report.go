package cli

import (
	"flag"
	"fmt"
	"sort"
	"strings"

	"github.com/cristobaltormo/git-sync/internal/github"
)

func cmdReport(cfgPath string, args []string) (int, error) {
	a, _, err := openApp(cfgPath, args, func(fs *flag.FlagSet) {})
	if err != nil {
		return 2, err
	}
	entries := a.eng.Entries()
	managed := map[string]bool{}
	for _, x := range entries {
		if x.Managed && x.GHOwner != "" {
			managed[strings.ToLower(x.GHOwner+"/"+x.GHName)] = true
		}
	}
	owners := map[string]bool{}
	for _, gh := range a.cfg.Accounts {
		owners[gh] = true
	}
	names := make([]string, 0, len(owners))
	for o := range owners {
		names = append(names, o)
	}
	sort.Strings(names)

	var only []github.Listed
	for _, o := range names {
		list, err := a.gh.ListRepos(o)
		if err != nil {
			return 1, err
		}
		for _, r := range list {
			if !managed[strings.ToLower(o+"/"+r.Name)] {
				only = append(only, r)
			}
		}
	}
	var pulls []github.Pull
	for _, o := range names {
		p, err := a.gh.OpenPulls(o)
		if err != nil {
			return 1, err
		}
		for _, x := range p {
			if managed[strings.ToLower(x.Owner+"/"+x.Repo)] {
				pulls = append(pulls, x)
			}
		}
	}

	heading("Pull requests open on the mirrors")
	if len(pulls) == 0 {
		fmt.Println(paint(" none", dim))
	} else {
		t := &table{head: []string{"PULL REQUEST", "AUTHOR", "TITLE"}}
		for _, p := range pulls {
			t.add([]string{p.Key(), p.Author, clip(p.Title, 60)})
		}
		t.print()
		fmt.Println(paint(" gitsync never merges or closes these. A merge on GitHub would be overwritten by the next sync:\n apply the change on the source server instead.", dim))
	}
	fmt.Println()
	heading("Repositories that exist only on GitHub")
	if len(only) == 0 {
		fmt.Println(paint(" none", dim))
	} else {
		t := &table{head: []string{"REPOSITORY", "VISIBILITY", "LAST PUSH"}}
		for _, r := range only {
			vis := "public"
			if r.Private {
				vis = "private"
			}
			t.add([]string{r.FullName, vis, strings.SplitN(r.PushedAt, "T", 2)[0]})
		}
		t.print()
		fmt.Println(paint(" gitsync leaves these alone. To bring one under it, create it on the source with the same name and set sync.adopt_existing.", dim))
	}
	return 0, nil
}
