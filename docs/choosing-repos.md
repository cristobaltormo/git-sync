# Choosing what to sync

gitsync mirrors everything it can see by default. That is right when the git
server only holds your own work. It is not when other people can add you to their
projects, or when some of your repositories should stay where they are. This page
explains the controls and how they fit together.

The short version:

```
gitsync repos
```

opens a full-screen selector in your terminal. Move with the arrow keys, press
space to switch a repository on or off, `w` to write. No web page, nothing keeps
running afterwards, and the daemon picks the change up on its own.

## Three layers

| Layer | Where | What it is for |
|---|---|---|
| Rules | `[filter]` in `config.toml` | General policy: skip forks, only repos where you are admin, what to do with new ones |
| Decisions | `repos.toml`, next to the config | "This repository: yes" or "no", written by `gitsync repos` |
| Options | `repos.toml` | Per-repository overrides: GitHub name, keep private, tags, metadata, what to do on removal |

`repos.toml` is plain TOML, safe to edit by hand, and written atomically.
[`examples/repos.example.toml`](../examples/repos.example.toml) shows every key.

A repository is judged in this order, and the first match wins:

1. A decision in `repos.toml`. `sync = true` syncs it even if a filter would skip
   it, `sync = false` never syncs it.
2. `skip_forks`, `skip_mirrors`, `skip_archived`, `skip_private`, `topic` and
   `scope`.
3. `include` and `exclude`.
4. `new_repos`, for repositories gitsync has not synced before.

`gitsync why <repo>` prints the answer for one repository and the exact command to
change it.

## Repositories that appear later

`filter.new_repos` decides what happens to a repository gitsync has never synced:

| Value | Behaviour |
|---|---|
| `sync` | It starts syncing right away. This is the default and the behaviour of every earlier version. |
| `review` | It waits. Nothing is created or pushed on GitHub, it shows up as *waiting* in `gitsync repos`, and a notification is sent if you configured one. You decide with one key. |
| `ignore` | It is left alone silently. Only repositories you picked are synced. |

When you switch to `review` or `ignore`, repositories that gitsync already syncs
keep syncing. The setting only applies to repositories it has not touched yet, so
turning it on never stops a running mirror.

If you work with other people, `review` is the setting to use: being added to a
project does not publish it anywhere until you say so.

## Profiles

A profile is a named set of defaults, so you do not have to know which setting to
pick. `gitsync init` asks for one, and `gitsync config profile <name>` applies one
later. A profile only changes the few settings it names.

| Profile | For | Sets |
|---|---|---|
| `personal` | A server that holds only your work | everything syncs, new repositories start right away, removals delete the GitHub copy |
| `team` | Others can add you to projects | new repositories wait for you, only repositories where you are admin, forks skipped, removals archive |
| `careful` | You want to hand-pick | nothing new is synced, GitHub copies are never made public by themselves, removals archive |

## Owner or collaborator

`filter.scope = "admin"` limits syncing to repositories where you are an
administrator. Repositories where you are only a collaborator are left alone.

The role comes from the permissions the server reports for your token, which
Forgejo, Gitea, Gogs and Codeberg include. Other servers do not report it in the
repository listing; there the role is unknown and a repository is not held
against it. An administrator token of a Forgejo or Gitea instance reports admin
on every repository of the instance, so `scope` is meant for the token of a
regular user.

## Per-repository options

Set them in `gitsync repos` (`o` on a repository), or from the command line:

```
gitsync repos set alice/site name=site-mirror keep_private=true
gitsync repos set alice/site tags=false on_delete=archive
gitsync repos set alice/site prune=false
gitsync repos set alice/site keep_private=       # clears one option
```

| Option | Effect |
|---|---|
| `name` | Name of the repository on GitHub. Changing it later renames the existing copy in place. |
| `keep_private` | The GitHub copy never becomes public, even if the source is. |
| `tags` | Mirror tags for this repository, overriding `sync.tags`. |
| `metadata` | Copy description and topics, overriding `sync.metadata`. |
| `prune` | `false` keeps branches that exist only on GitHub. By default the push removes them so GitHub equals the source. |
| `on_delete` | `delete`, `archive` or `ignore` when it disappears from the source. |
| `note` | Free text for you. |

Stopping to sync a repository never deletes its GitHub copy. gitsync leaves it as
it is and only stops updating it.

## Pull requests and repositories created on GitHub

gitsync is one way: the source is the truth and GitHub is a copy. That has two
consequences worth knowing.

A pull request opened on a mirror cannot be merged there, because the next sync
would overwrite the merge. gitsync never merges, closes or rejects a pull
request. What it can do is tell you:

```toml
[pull_requests]
mode = "comment"     # "leave" (default) does nothing on GitHub
message = "This is a read-only mirror. Please send changes to {url}."

[notify]
url = "https://ntfy.example.com/gitsync"
format = "text"      # json, text, slack or discord
events = ["pending", "failing", "pull_request"]
```

Pull requests are looked up with one GitHub search request per account every
`verify_interval`, and only if `mode = "comment"` or a notification is configured
for them. The first look after enabling it records what is already open without
reacting, so you are not flooded. `{url}` and `{repo}` are replaced in the message.

If a public repository takes pull requests, two things can still affect them. gitsync
force-pushes and prunes, so a branch that exists only on GitHub is removed on the next
sync, and GitHub closes a pull request whose head branch disappears (pull requests
from forks are not affected). Set `prune = false` on that repository to keep such
branches, and `on_delete = "ignore"` so that removing the source repository never
deletes the GitHub one together with its pull requests and issues. gitsync itself
never closes, deletes or edits pull requests, issues, discussions or releases.

A repository created directly on GitHub, or one you were invited to there, is not
touched in any way. `gitsync report` lists the pull requests open on your mirrors
and the repositories that exist only on GitHub, so nothing goes unnoticed.

## Notifications

`[notify]` sends one message when something needs you:

| Event | When |
|---|---|
| `pending` | A new repository is waiting for your decision (`new_repos = "review"`) |
| `failing` | A repository has failed three times in a row |
| `pull_request` | A pull request was opened on a mirror |

`format` is `json` (a small object), `text` (plain body, works with ntfy), `slack`
or `discord`. Delivery is best effort with a ten second timeout, nothing is queued,
and nothing is sent in `--dry-run`. `gitsync config test-notify` sends a sample.

## Changing things while it runs

The daemon looks at the modification time of `config.toml` and `repos.toml`
whenever it polls (a `stat` call) and reloads when either changed, so a change
made by `gitsync repos`, `gitsync config` or by hand applies within
`poll_interval` seconds. With `poll_interval = 0`, or after changing `listen.*`,
use `systemctl reload gitsync` or restart. A file with a mistake is rejected as a
whole and the previous settings stay active.

## Cost

Nothing here runs unless you use it. `gitsync repos` is a separate process that
exists only while the selector is open. The daemon adds two `stat` calls per poll.
Pull request lookups happen at most once per `verify_interval` and only when
enabled. The selector is built on the standard library and `golang.org/x/term`.

## Commands

| | |
|---|---|
| `gitsync repos` | the selector (a list in a pipe or script) |
| `gitsync repos list [--status S] [--json]` | every repository and what happens to it |
| `gitsync repos allow <repo>... \| --pending` | sync these |
| `gitsync repos ignore <repo>...` | never sync these |
| `gitsync repos reset <repo>...` | forget the decision |
| `gitsync repos set <repo> key=value ...` | per-repository options |
| `gitsync why <repo>` | why a repository is or is not synced |
| `gitsync plan [--json]` | what the next sync would do, changing nothing |
| `gitsync report` | pull requests on mirrors, repositories only on GitHub |
| `gitsync config show [--all]` / `get` / `set` / `edit` / `path` | settings, with comments kept and validation before saving |
| `gitsync config profile [name]` | list or apply a profile |
| `gitsync config test-notify` | send a sample notification |

A repository is `owner/name`, just the name, or a pattern such as `my-org/*`.
