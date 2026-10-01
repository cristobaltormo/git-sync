package cli

import (
	"fmt"
	"time"

	"github.com/cristobaltormo/git-sync/internal/config"
)

func yn(b bool) string {
	if b {
		return "yes"
	}
	return "no"
}

func cmdWhy(cfgPath string, args []string) (int, error) {
	a, fs, err := openApp(cfgPath, args, nil)
	if err != nil {
		return 2, err
	}
	if fs.NArg() != 1 {
		return 2, config.Errorf("usage: gitsync why <repo>")
	}
	items, err := inventory(a)
	if err != nil {
		return 1, err
	}
	found, err := resolve(items, fs.Args())
	if err != nil {
		return 2, err
	}
	for n, it := range found {
		if n > 0 {
			fmt.Println()
		}
		explain(a, it)
	}
	return 0, nil
}

func explain(a *app, it *item) {
	r, v := it.Repo, it.Verdict
	heading(it.full())
	verdict := map[string]string{
		stSynced:   "synced to GitHub",
		stError:    "synced, but the last attempt failed",
		stQueued:   "selected, will be created on GitHub on the next pass",
		stWaiting:  "waiting for your decision",
		stIgnored:  "ignored, by your choice",
		stFiltered: "not synced",
	}[it.Status]
	fmt.Printf("  %s %s\n", pad("Status", 14), paint(verdict, statusStyle(it.Status), bold))
	if v.Why != "" {
		fmt.Printf("  %s %s\n", pad("Because", 14), v.Why)
	} else if it.Status == stSynced || it.Status == stQueued || it.Status == stError {
		fmt.Printf("  %s %s\n", pad("Because", 14), "no rule excludes it")
	}
	role := r.Role
	if role == "" {
		role = "unknown for this server"
	}
	fmt.Printf("  %s private %s, fork %s, pull mirror %s, archived %s, your role: %s\n", pad("On the source", 14),
		yn(r.Private), yn(r.Fork), yn(r.Mirror), yn(r.Archived), role)
	if len(r.Topics) > 0 {
		fmt.Printf("  %s %v\n", pad("Topics", 14), r.Topics)
	}
	cfg := a.cfg
	dst, ok := cfg.GitHubOwner(r.Owner)
	name := r.Name
	rule := v.Rule
	if rule != nil && rule.Name != "" {
		name = rule.Name
	}
	if ok {
		fmt.Printf("  %s %s/%s\n", pad("Goes to", 14), dst, name)
	}
	opts := []string{
		"tags " + yn(ruleBool(rule, func(x *config.RepoRule) *bool { return x.Tags }, cfg.Sync.Tags)),
		"description and topics " + yn(ruleBool(rule, func(x *config.RepoRule) *bool { return x.Metadata }, cfg.Sync.Metadata)),
		"follows visibility " + yn(cfg.Sync.Visibility && !ruleBool(rule, func(x *config.RepoRule) *bool { return x.KeepPrivate }, false)),
	}
	onDelete := cfg.Sync.OnDelete
	if rule != nil && rule.OnDelete != "" {
		onDelete = rule.OnDelete
	}
	opts = append(opts, "if removed: "+onDelete)
	fmt.Printf("  %s %s\n", pad("Options", 14), join(opts))
	if it.Has && it.Entry.LastOK > 0 {
		fmt.Printf("  %s %s\n", pad("Last sync", 14), ago(time.Now().Unix()-it.Entry.LastOK))
	}
	if it.Entry.LastError != "" {
		fmt.Printf("  %s %s\n", pad("Last error", 14), paint(it.Entry.LastError, red))
	}
	if it.Entry.Pending != 0 {
		fmt.Printf("  %s since %s\n", pad("Waiting", 14), time.Unix(it.Entry.Pending, 0).Format("2006-01-02 15:04"))
	}
	fmt.Println()
	switch it.Status {
	case stWaiting:
		fmt.Printf("  To sync it:    gitsync repos allow %s\n  To skip it:    gitsync repos ignore %s\n", it.full(), it.full())
	case stIgnored:
		fmt.Printf("  To sync it:    gitsync repos allow %s\n  To undo this:  gitsync repos reset %s\n", it.full(), it.full())
	case stFiltered:
		fmt.Printf("  To sync it anyway:  gitsync repos allow %s\n", it.full())
	default:
		fmt.Printf("  To stop syncing it:  gitsync repos ignore %s\n", it.full())
	}
}

func ruleBool(r *config.RepoRule, pick func(*config.RepoRule) *bool, def bool) bool {
	if r != nil {
		if p := pick(r); p != nil {
			return *p
		}
	}
	return def
}

func join(parts []string) string {
	out := ""
	for i, p := range parts {
		if i > 0 {
			out += "; "
		}
		out += p
	}
	return out
}
