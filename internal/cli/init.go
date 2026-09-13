package cli

import (
	"bufio"
	"crypto/rand"
	"encoding/hex"
	"encoding/json"
	"flag"
	"fmt"
	"os"
	"os/exec"
	"slices"
	"strconv"
	"strings"

	"github.com/cristobaltormo/git-sync/internal/config"
	"github.com/cristobaltormo/git-sync/internal/forge"
	"github.com/cristobaltormo/git-sync/internal/github"
	"github.com/cristobaltormo/git-sync/internal/logx"
)

var stdin = bufio.NewReader(os.Stdin)

func isTerminal() bool {
	st, err := os.Stdin.Stat()
	return err == nil && st.Mode()&os.ModeCharDevice != 0
}

func ask(prompt, def string, secret bool) string {
	label := prompt + ": "
	if def != "" {
		label = fmt.Sprintf("%s [%s]: ", prompt, def)
	}
	fmt.Fprint(os.Stderr, label)
	hide := secret && isTerminal()
	if hide {
		exec.Command("stty", "-echo").Run()
	}
	line, _ := stdin.ReadString('\n')
	if hide {
		exec.Command("stty", "echo").Run()
		fmt.Fprintln(os.Stderr)
	}
	if line = strings.TrimSpace(line); line != "" {
		return line
	}
	return def
}

func yes(prompt string, def bool) bool {
	hint := " (Y/n)"
	if !def {
		hint = " (y/N)"
	}
	v := strings.ToLower(ask(prompt+hint, "", false))
	if v == "" {
		return def
	}
	return strings.HasPrefix(v, "y")
}

func quote(s string) string {
	b, _ := json.Marshal(s)
	return string(b)
}

const template = `# gitsync configuration. Every option is explained in examples/config.example.toml.

[source]
type = %s
url = %s
token = %s

[github]
token = %s

# source owner = GitHub owner
[accounts]
%s

[listen]
host = "127.0.0.1"
port = %d
secret = %s

[sync]
visibility = %t
on_delete = %s
`

func cmdInit(cfgPath string, args []string) (int, error) {
	var force *bool
	path, _, err := flags("init", cfgPath, args, func(fs *flag.FlagSet) {
		force = fs.Bool("force", false, "overwrite an existing config")
	})
	if err != nil {
		return 2, err
	}
	if path == "" {
		path = "config.toml"
	}
	if _, err := os.Stat(path); err == nil && !*force {
		return 2, config.Errorf("%s already exists (use --force to overwrite)", path)
	}
	fmt.Printf("This asks a few questions and writes %s. Nothing is changed on either side yet.\n\n", path)

	typ := strings.ToLower(ask("Source server type ("+strings.Join(config.SourceTypes, ", ")+")", "forgejo", false))
	if !slices.Contains(config.SourceTypes, typ) {
		return 2, config.Errorf("type must be one of: %s", strings.Join(config.SourceTypes, ", "))
	}
	tmp := config.Default()
	tmp.Source.Type = typ
	url := strings.TrimRight(ask("Source URL as this machine reaches it (e.g. https://git.example.com)", config.DefaultURL(typ), false), "/")
	fmt.Println("Create an access token on the source server with read access to the repositories,\n" +
		"and write access to their webhooks (admin if you want one system-wide webhook).")
	srcToken := ask("Source token", "", true)
	port, err := strconv.Atoi(ask("Port for the webhook listener", "9001", false))
	if err != nil {
		return 2, config.Errorf("that is not a port number")
	}
	buf := make([]byte, 24)
	rand.Read(buf)
	secret := hex.EncodeToString(buf)

	tmp.Source.URL, tmp.Source.Token = url, srcToken
	logx.Hide(srcToken)
	src, err := forge.New(tmp)
	if err != nil {
		return 2, config.Errorf("%v", err)
	}
	me, err := src.Whoami()
	if err != nil {
		return 2, config.Errorf("could not log in to the source: %v", err)
	}
	fmt.Printf("Logged in as %s.\n\n", me.Login)

	fmt.Println("Create a GitHub token (Settings, Developer settings, Personal access tokens, classic)\n" +
		"with the scopes 'repo' and 'delete_repo'.")
	ghToken := ask("GitHub token", "", true)
	logx.Hide(ghToken)
	tmp.GitHub.Token = ghToken
	ghLogin, _, err := github.New(tmp, false).Whoami()
	if err != nil {
		return 2, config.Errorf("could not log in to GitHub: %v", err)
	}
	fmt.Printf("Logged in to GitHub as %s.\n\n", ghLogin)

	var lines []string
	for _, o := range strings.Split(ask("Which owners to mirror (comma separated)", me.Login, false), ",") {
		o = strings.TrimSpace(o)
		if o == "" {
			continue
		}
		def := o
		if strings.EqualFold(o, me.Login) {
			def = ghLogin
		}
		lines = append(lines, fmt.Sprintf("%s = %s", quote(o), quote(ask("GitHub owner for "+o, def, false))))
	}
	vis := yes("Should public repos on the source become public on GitHub (private ones stay private)?", true)
	onDelete := "archive"
	if yes("When a repo is deleted on the source, delete it on GitHub too? (no = archive it)", true) {
		onDelete = "delete"
	}
	f, err := os.OpenFile(path, os.O_WRONLY|os.O_CREATE|os.O_TRUNC, 0o600)
	if err != nil {
		return 1, err
	}
	defer f.Close()
	fmt.Fprintf(f, template, quote(typ), quote(url), quote(srcToken), quote(ghToken),
		strings.Join(lines, "\n"), port, quote(secret), vis, quote(onDelete))
	fmt.Printf("\nWrote %s (only you can read it).\n", path)
	fmt.Println("Next: `gitsync check`, then `gitsync run --dry-run` to see what it would do,")
	fmt.Println("then `gitsync run`, or `sudo gitsync install` to keep it running.")
	return 0, nil
}
