# Configuration

gitsync reads two files: `config.toml`, with the connection to both sides and the
general rules, and `repos.toml`, with decisions and options for single
repositories. This page lists every key of both. The annotated examples are
[`examples/config.example.toml`](../examples/config.example.toml) and
[`examples/repos.example.toml`](../examples/repos.example.toml).

Only `[source]`, `[github]`, `[accounts]` and `listen.secret` are required.
`gitsync init` writes a minimal file, and everything left out takes the default
shown here.

## Where the files are

The config is the first of these that exists:

1. the file given with `-c` or `--config`;
2. the file named in the `GITSYNC_CONFIG` environment variable (the Docker image
   sets it to `/config/config.toml`);
3. `config.toml` in the current directory;
4. `/etc/gitsync/config.toml`, or `%ProgramData%\gitsync\config.toml` on Windows.

`repos.toml` is next to the config unless `paths.repos_file` says otherwise.

`gitsync config path` prints the file in use, `gitsync config show` the values
that differ from the defaults (`--all` for every one), and `gitsync config set
key value` changes one key, keeping the comments of the file and validating it
before saving.

## Rules for the file

- Section and key names are not case sensitive.
- An unknown key is an error, so a typo does not go unnoticed.
- A file with a mistake is rejected as a whole. When the daemon reloads it, the
  previous settings stay active.
- On Windows write paths between single quotes, `'C:\ProgramData\gitsync\source.token'`,
  because a backslash is an escape inside double quotes.
- Keep the file readable only by the user that runs gitsync. `gitsync init` and
  `gitsync install` set the permissions.

## Secrets

The source token, the GitHub token and the webhook secret can each come from three
places. The first one that is set wins:

| Secret | Environment variable | File | Key |
|---|---|---|---|
| source token | `GITSYNC_SOURCE_TOKEN` | `source.token_file` | `source.token` |
| GitHub token | `GITSYNC_GITHUB_TOKEN` | `github.token_file` | `github.token` |
| webhook secret | `GITSYNC_WEBHOOK_SECRET` | `listen.secret_file` | `listen.secret` |

Values from the environment and from files are trimmed. A secret that contains
control characters is refused.

## config.toml

### `[source]`

| Key | Type | Default | Meaning |
|---|---|---|---|
| `type` | string | `"forgejo"` | `forgejo`, `gitea`, `gogs`, `codeberg`, `gitlab`, `gitbucket`, `onedev`, `bitbucket-cloud` or `bitbucket-server` (experimental). |
| `url` | string | see meaning | Base URL of the server as this machine reaches it, used for the API and for git. `http://127.0.0.1:3000` is fine on the same host. Defaults to `https://codeberg.org`, `https://gitlab.com` and `https://bitbucket.org` for those types; required for the others. |
| `token` | string | `""` | API token of the source server. Read access to the repositories is enough to mirror; webhooks also need write access to them, and an admin token lets Forgejo, Gitea and GitLab use one system-wide webhook. Required, from here, `token_file` or the environment. |
| `token_file` | string | `""` | File that holds the token. |
| `user` | string | `""` | User name for the providers that need one next to the token. |

### `[github]`

| Key | Type | Default | Meaning |
|---|---|---|---|
| `token` | string | `""` | Classic token with the scopes `repo` and `delete_repo` (the second only if `on_delete = "delete"`), or a fine-grained token with Administration and Contents write on the target repositories. Required. |
| `token_file` | string | `""` | File that holds the token. |
| `api_url` | string | `"https://api.github.com"` | Change only for GitHub Enterprise Server. |
| `git_url` | string | `"https://github.com"` | Change only for GitHub Enterprise Server. |

### `[accounts]`

One line per source owner, mapping it to a GitHub owner. Both sides can be a user
or an organization, and at least one line is required.

```toml
[accounts]
alice = "alice"
my-org = "my-org"
```

A token can only create repositories under its own user or under organizations it
belongs to. What counts as an owner depends on the server: user or organization
names on Forgejo, Gitea, Gogs and GitBucket, namespaces on GitLab
(`group/subgroup`), workspaces on Bitbucket Cloud, project keys on Bitbucket
Server, and the parent project path on OneDev (`"/"` for projects at the root).
See [providers.md](providers.md).

### `[listen]`

| Key | Type | Default | Meaning |
|---|---|---|---|
| `host` | string | `"127.0.0.1"` | Where the webhook listener binds. Keep it on localhost when the git server runs on the same machine. `0.0.0.0` in a container. An empty value also listens on IPv6 where the system supports dual stack. |
| `port` | integer | `9001` | Port of the listener, 1 to 65535. |
| `secret` | string | `""` | Shared secret. The git server signs every webhook with it, and requests that do not carry it are dropped. Required; `gitsync init` generates one. |
| `secret_file` | string | `""` | File that holds the secret. |
| `public_url` | string | `""` | URL the git server should call. Empty means `http://<host>:<port>/hook`, with `127.0.0.1` when `host` is `0.0.0.0`. Set it when the git server reaches this machine under another name, for example `http://gitsync:9001/hook` in Docker. |

### `[sync]`

| Key | Type | Default | Meaning |
|---|---|---|---|
| `poll_interval` | integer, seconds | `5` | Time between polls of the git server. The poll notices visibility, renames, descriptions, archiving and deletions, and catches pushes whose webhook got lost. `0` turns it off: only webhooks remain, plus a safety listing every 5 minutes. |
| `verify_interval` | integer, seconds | `3600` | Time between full checks against GitHub: each repository is compared with what is really there, and those that changed or were not pushed in a day are pushed again. `0` disables them. |
| `workers` | integer | `2` | Repositories synced in parallel. At least 1. |
| `visibility` | boolean | `true` | Make GitHub public or private follow the source. New repositories are always created private and opened only after their content is pushed, and private repositories never become public. |
| `metadata` | boolean | `true` | Copy description, website and topics. |
| `default_branch` | boolean | `true` | Follow the default branch. |
| `archived` | boolean | `true` | Follow the archived flag. |
| `tags` | boolean | `true` | Mirror tags as well as branches. |
| `on_delete` | string | `"delete"` | What to do on GitHub when a repository is removed from the source: `delete` (needs the `delete_repo` scope), `archive` (keeps the code) or `ignore`. |
| `delete_grace` | integer, seconds | `10` | A removal is acted on only after the repository has been missing this long and asking the server for it directly confirms it is gone. |
| `delete_limit` | integer | `5` | Most repositories deleted or archived per hour. `0` means no limit. |
| `adopt_existing` | boolean | `false` | Take over a GitHub repository that gitsync did not create. Its content is overwritten to match the source, and from then on it is treated like one gitsync created. |
| `dry_run` | boolean | `false` | Log what would happen and change nothing, like `gitsync run --dry-run`. |
| `git_timeout` | integer, seconds | `1800` | Time before a single git command is given up on. `0` means no limit. Independent of this, a git command that writes nothing for 15 seconds is treated as stalled and restarted. |

Integers must be 0 or more.

### `[filter]`

| Key | Type | Default | Meaning |
|---|---|---|---|
| `include` | list of strings | `[]` | Glob patterns matched against `owner/name`, or against the name alone when the pattern has no slash. Empty means everything. |
| `exclude` | list of strings | `[]` | Same patterns; a match is skipped. |
| `topic` | string | `""` | Only sync repositories that carry this topic on the source. The topic itself is not copied to GitHub. |
| `skip_forks` | boolean | `false` | Skip forks. |
| `skip_mirrors` | boolean | `true` | Skip repositories that are pull mirrors of something else (Forgejo, Gitea, GitLab). |
| `skip_archived` | boolean | `false` | Skip archived repositories. |
| `skip_private` | boolean | `false` | Sync public repositories only. |
| `scope` | string | `"all"` | `all`, or `admin` to leave alone the repositories where the token's user is only a collaborator. Forgejo, Gitea, Gogs and Codeberg report the role; elsewhere it is unknown and the repository is kept. |
| `new_repos` | string | `"sync"` | What happens to a repository gitsync has never synced: `sync` starts right away, `review` waits for your decision and sends a notification, `ignore` leaves it alone so only what you pick is synced. Repositories already synced keep syncing when you change it. |

The order in which these are applied is in [choosing-repos.md](choosing-repos.md#three-layers).

### `[hooks]`

| Key | Type | Default | Meaning |
|---|---|---|---|
| `mode` | string | `"auto"` | `auto`: one system-wide webhook when the token is an admin (Forgejo, Gitea, GitLab), otherwise one per repository, created when it is first synced. `system`: only the system-wide webhook. `repo`: only per-repository webhooks. `none`: do not touch the git server; the poll catches everything and a push takes up to `poll_interval` seconds longer. |

### `[paths]`

| Key | Type | Default | Meaning |
|---|---|---|---|
| `state_dir` | string | see meaning | Where `state.json` and the bare mirrors live. Empty means `$STATE_DIRECTORY` when set (systemd and the Docker image set it), `%LOCALAPPDATA%\gitsync` on Windows, `/var/lib/gitsync` as root, and `$XDG_STATE_HOME/gitsync` or `~/.local/state/gitsync` otherwise. The Windows installer uses `C:\ProgramData\gitsync\state`. |
| `repos_file` | string | `""` | Where per-repository decisions live. Empty means `repos.toml` next to the config. |

### `[log]`

| Key | Type | Default | Meaning |
|---|---|---|---|
| `level` | string | `"info"` | `debug`, `info`, `warn` or `error`. `debug` logs every webhook received. Secrets are replaced by `***` at every level. |

### `[notify]`

| Key | Type | Default | Meaning |
|---|---|---|---|
| `url` | string | `""` | An http(s) URL that receives a POST when something needs you. Empty means no notifications. |
| `format` | string | `"json"` | `json`, `text` (plain body, works with ntfy), `slack` or `discord`. |
| `events` | list of strings | `["pending", "failing", "pull_request"]` | `pending`: a new repository waits for review. `failing`: a repository failed three times in a row. `pull_request`: a pull request was opened on a mirror. |

`gitsync config test-notify` sends a sample. Nothing is sent in dry-run.

### `[pull_requests]`

| Key | Type | Default | Meaning |
|---|---|---|---|
| `mode` | string | `"leave"` | What to do when a pull request is opened on a mirror: `leave` does nothing on GitHub, `comment` adds one note saying the repository is a read-only mirror. gitsync never merges or closes pull requests. |
| `message` | string | `""` | Text of the note. `{url}` and `{repo}` are replaced with the source repository. Empty uses a built-in text that says the repository is a read-only mirror and asks to send the change to the maintainer. |

## repos.toml

One table per repository, keyed by `owner/name` on the source in lowercase.
`gitsync repos` writes it, and it is safe to edit by hand. A running gitsync
reloads it by itself.

```toml
[repos."my-org/internal-tools"]
sync = true
name = "tools-mirror"
keep_private = true
tags = false
```

| Key | Type | Default | Meaning |
|---|---|---|---|
| `sync` | boolean | unset | `true` syncs the repository even if a filter would skip it; `false` never syncs it. Unset leaves the decision to the rules. |
| `name` | string | source name | Name of the repository on GitHub: letters, digits, `.`, `_` and `-`, up to 100 characters. Changing it later renames the existing copy in place. |
| `keep_private` | boolean | unset | `true` keeps the GitHub copy private even if the source is public. |
| `tags` | boolean | `sync.tags` | Mirror tags for this repository. |
| `metadata` | boolean | `sync.metadata` | Copy description and topics for this repository. |
| `prune` | boolean | `true` | `false` keeps branches that exist only on GitHub, for example the head branch of an open pull request. |
| `on_delete` | string | `sync.on_delete` | `delete`, `archive` or `ignore` when the repository disappears from the source. |
| `note` | string | `""` | Free text for you. |

From the command line:

```sh
gitsync repos set alice/site name=site-mirror keep_private=true
gitsync repos set alice/site keep_private=     # clears one option
```

## Environment variables

| Variable | Effect |
|---|---|
| `GITSYNC_CONFIG` | Config file to use when `-c` is not given. |
| `GITSYNC_SOURCE_TOKEN`, `GITSYNC_GITHUB_TOKEN`, `GITSYNC_WEBHOOK_SECRET` | Secrets, see above. |
| `STATE_DIRECTORY` | Default state directory. systemd sets it from `StateDirectory=`. |
| `HTTPS_PROXY`, `HTTP_PROXY`, `NO_PROXY` | Proxy for API requests and notifications, also in lowercase. git commands follow git's own proxy configuration. |
| `SSL_CERT_FILE`, `SSL_CERT_DIR` | On Linux, where to read the trusted certificates from instead of the usual system locations. |
| `NO_COLOR` | Turns off colours in the terminal output. |
