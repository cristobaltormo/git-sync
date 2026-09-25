# gitsync

English | [Español](README.es.md)

[![CI](https://github.com/cristobaltormo/git-sync/actions/workflows/ci.yml/badge.svg)](https://github.com/cristobaltormo/git-sync/actions/workflows/ci.yml)
[![License](https://img.shields.io/badge/license-Apache--2.0-blue)](LICENSE)

Mirror your repositories from a self-hosted git server to GitHub, and keep them
in step. A push lands on GitHub within a few seconds. Renaming a repo, making it
public, changing its description, archiving it or deleting it is picked up just
as fast.

gitsync is a single static binary. Idle it uses about 11 MB of RAM on Linux and
next to no CPU, it needs no cron and no runtime, only `git`. It runs on Linux,
macOS, Windows and in Docker, and its tests run on the three systems on every
push.

## Supported servers

| Server | Status | Webhooks |
|---|---|---|
| Forgejo, Codeberg | tested | one system webhook (admin token) or per repo |
| Gitea | tested | per repo (a system webhook needs a Gitea restart, see the guide) |
| GitLab, self-hosted and gitlab.com | tested | one system webhook (admin token) or per repo |
| Gogs | tested | per repo |
| GitBucket | tested | per repo |
| OneDev | tested | per repo |
| Bitbucket Cloud | tested | per repo, Bitbucket has to reach your server (see the guide) |
| Bitbucket Server and Data Center | experimental | per repo |

Tested means every step of the sync was run against a real server: Forgejo 16,
Gitea 1.27, GitLab 19, Gogs 0.14, GitBucket 4.48, OneDev 16 and Bitbucket
Cloud. Bitbucket Server and Data Center were written from their documentation and
have only been exercised against a simulated server, since running them needs a
licence. What each server can and cannot do is in
[docs/providers.md](docs/providers.md).

## What is instant and what is not

Git servers send a webhook when someone pushes or creates or deletes a branch
or tag. Most of them send nothing when a repo is renamed, made public or
private, archived, retitled or deleted. gitsync listens for webhooks and also
polls.

| Change on the git server | Noticed through | On GitHub after |
|---|---|---|
| push, new or deleted branch, new tag | webhook | 2 to 5 seconds |
| new repo | webhook or poll | 3 to 5 seconds |
| visibility, description, website, topics, default branch, archived, rename | poll, or webhook where the server sends one | up to 5 seconds, plus the time to apply it |
| repo deleted | poll, then a confirmation | about 15 seconds |

The poll lists your repos every 5 seconds and compares the result with the
previous listing. That is about 3 KB per repo and a few milliseconds on the
server. It does not depend on what a given server updates when something is
edited, it compares the fields that get mirrored. When something differs only
that repo is synced, and if a webhook ever gets lost the poll still catches the
push. `poll_interval` can be lowered, or set to 0 to rely on webhooks alone.

Measured latencies for each server are in [docs/benchmarks.md](docs/benchmarks.md).

## Install

Download the binary for your machine from the
[releases page](https://github.com/cristobaltormo/git-sync/releases):

```
curl -L -o gitsync https://github.com/cristobaltormo/git-sync/releases/latest/download/gitsync-linux-amd64
chmod +x gitsync
```

On Windows, in PowerShell:

```
Invoke-WebRequest https://github.com/cristobaltormo/git-sync/releases/latest/download/gitsync-windows-amd64.exe -OutFile gitsync.exe
```

There are builds for Linux (amd64, arm64, armv7), macOS (Intel and Apple
silicon) and Windows (amd64 and arm64). To build it yourself you need Go 1.24 or
newer:

```
go install github.com/cristobaltormo/git-sync/cmd/gitsync@latest
```

`git` 2.32 or newer has to be installed on the machine ([Git for Windows](https://gitforwindows.org) on Windows).

## Set it up

```
gitsync init      # asks a few questions and writes config.toml
gitsync check     # tests both tokens, their permissions and every account
gitsync run --dry-run
```

`run --dry-run` logs what it would do and changes nothing on either side. When
that looks right:

```
sudo gitsync install
```

This creates a `gitsync` user, copies the binary to `/usr/local/bin` and the
config to `/etc/gitsync/config.toml`, and starts a systemd service. Logs are in
`journalctl -u gitsync -f`. After editing the config, `systemctl reload gitsync`
applies it without a restart.

On Windows, run `gitsync install` from a PowerShell opened as administrator. It
copies the binary to `C:\Program Files\gitsync` and the config to
`C:\ProgramData\gitsync`, restricted to administrators, and registers a
scheduled task that starts with Windows, runs as SYSTEM and is started again
within a minute if it ever stops. If the webhook listener is not on localhost it
also opens that port in the Windows firewall. The log is
`C:\ProgramData\gitsync\gitsync.log` and the mirrors are in
`C:\ProgramData\gitsync\state`. There is no `kill -HUP` on Windows: after
editing the config run `gitsync install` again, or `schtasks /End /TN gitsync`
and let the task start it. `gitsync uninstall` removes the task, the firewall
rule and the binary.

On macOS there is no installer: keep `gitsync run` alive with launchd, or use
Docker.

### Tokens

On the git server, create an access token that can read the repositories and
manage their webhooks. If it belongs to an admin, gitsync creates one
system-wide webhook that covers every repo, including the ones you create later,
on the servers that have them. Otherwise it creates one webhook per repo.

On GitHub, a classic personal access token with the scopes `repo` and
`delete_repo`. Leave out `delete_repo` if you set `on_delete` to `"archive"` or
`"ignore"`. A fine-grained token works too if it has Administration and Contents
write access to the repos.

### If webhooks never arrive

Most servers refuse to call addresses on private networks or on the machine
itself unless you allow it. The setting for each server is in
[docs/providers.md](docs/providers.md). If you would rather not touch the git
server's config, set `hooks.mode = "none"`: everything keeps working through the
poll, only a push takes 4 to 8 seconds to show up instead of 2 to 5.

## Configuration

[`examples/config.example.toml`](examples/config.example.toml) lists every option
with its default and what it does. These are the ones people usually change:

```toml
[accounts]
alice = "alice"                # source owner = GitHub owner
my-org = "my-github-org"       # organizations work too

[filter]
skip_private = true            # publish public repos only
topic = "github"               # or: only repos that carry this topic
exclude = ["scratch-*", "alice/dotfiles"]

[sync]
on_delete = "archive"          # "delete", "archive" or "ignore"
visibility = false             # never touch public/private on GitHub
```

The `topic` filter lets you decide what goes to GitHub from the git server's own
UI: add the topic to a repo and it starts syncing, remove it and it stops. The
topic itself is not copied to GitHub.

## Things worth knowing

The GitHub copy is exact. Branches and tags are force-pushed and pruned, so
GitHub always equals the source and whatever you do directly on the GitHub copy
gets overwritten. Only git data is mirrored, plus description, website, topics,
default branch, visibility and the archived flag, where the server provides them.
Pull requests, issues, wikis, releases and LFS objects are not mirrored.

New repos are created private on GitHub, pushed, and only then opened up if the
source is public. A repo that is private on the source never becomes public.

A repo that already exists on GitHub and was not created by gitsync is left
alone and reported in the log. Set `adopt_existing = true` to let gitsync take
it over, knowing the GitHub copy will be overwritten to match the source.

Deleting is careful on purpose. A repo that disappears from the source is only
acted on after `delete_grace` seconds, and only if asking the server for it
directly confirms it is gone, because a token that lost access looks exactly like
a deleted repo. At most `delete_limit` repos are deleted per hour, and a repo
owned by someone else on GitHub is never touched. `on_delete = "archive"` keeps
the code instead.

Renames are applied to the GitHub repo in place, so stars and links survive. If
the new name is already taken on GitHub, gitsync stops and says so.

GitHub applies visibility changes slowly. Making a repo public and private again
within seconds gets you "a previous visibility change is still in progress" and
rejected pushes for a while. gitsync recognises that, waits ten seconds and tries
again.

GitHub rejects pushes that contain a token it recognises. The error is in the
log and the repo is retried with growing delays until the secret is gone from the
history.

## Docker

From a clone of this repository:

```
docker build -t gitsync .
docker run -d --name gitsync --restart unless-stopped \
  -v ./config.toml:/config/config.toml:ro -v gitsync-data:/data \
  gitsync
```

In a container set `listen.host = "0.0.0.0"` and `listen.public_url` to the
address the git server uses to reach it, for example `http://gitsync:9001/hook`
when both are on the same Docker network. [`compose.yaml`](compose.yaml) is a
starting point. When the git server is on another machine, publish the port
(`-p 9001:9001`) and use that machine's address in `public_url`. Tokens can come
from the environment instead of the file: `GITSYNC_SOURCE_TOKEN`,
`GITSYNC_GITHUB_TOKEN` and `GITSYNC_WEBHOOK_SECRET`.

## Commands

| | |
|---|---|
| `gitsync init` | write a config file, asking questions |
| `gitsync check` | validate the config, tokens, scopes and accounts |
| `gitsync run [--dry-run]` | the daemon |
| `gitsync sync [owner/name ...]` | mirror once and exit, everything or just some repos |
| `gitsync status` | each repo, where it goes, when it last synced, last error |
| `gitsync hooks [--force] [--remove]` | create, rewrite or remove the webhooks |
| `gitsync install` / `uninstall` | systemd service on Linux, scheduled task on Windows |

`-c file.toml` picks another config. Without it, `./config.toml` and then
`/etc/gitsync/config.toml` are tried. `kill -HUP` reloads the config.

## Resource use

Idle: around 11 MB of RAM, about 10 ms of CPU per minute with 20 repos, one HTTP
request every `poll_interval` seconds. Syncing is where `git` uses memory, and it
is told to keep it low. The systemd unit has a hard cap of 512 MB that you can
change. Numbers and method are in [docs/benchmarks.md](docs/benchmarks.md).

State is one small JSON file plus a bare mirror of each repo under
`/var/lib/gitsync` (`C:\ProgramData\gitsync\state` on Windows). The mirrors can be deleted at any time and are fetched again
on the next sync.

## Troubleshooting

`gitsync check` catches most setup problems. After that `gitsync status` shows
the last error of each repo, and `log.level = "debug"` logs every webhook
received.

- Pushes take several seconds: the webhook is not arriving, see "If webhooks
  never arrive".
- `already exists on GitHub and gitsync did not create it`: see `adopt_existing`.
- `the token belongs to 'x' and cannot create repos under the user 'y'`: a token
  can only create repos in its own account or in organizations it belongs to. Map
  that source owner to the token's own login or to an organization.
- A repo you deleted is still on GitHub: run `gitsync check` and look for the
  `delete_repo` scope.

## Development

```
make test      # go vet and go test -race
make build
make release   # cross-compiled binaries and checksums in dist/
```

See [docs/architecture.md](docs/architecture.md) for how it is put together and
[CONTRIBUTING.md](CONTRIBUTING.md) for adding a server.

## Questions

Use [GitHub Discussions](https://github.com/cristobaltormo/git-sync/discussions) for
questions and ideas, and issues for bugs. `gitsync check` output and the log
with `log.level = "debug"` (tokens are hidden) make a report much easier to act on.

## Credits

The idea for this project, and its first version, came from
[Mario Gómez](https://github.com/mariogdn), who wrote an initial script that
pushed repositories to GitHub every hour. gitsync was developed from there by
Cristóbal Tormo.

## License

Apache License 2.0, see [LICENSE](LICENSE) and [NOTICE](NOTICE).
