# Changelog

## 2.0.0 (2026-11-13)

gitsync is rewritten in Rust. Commands, flags, configuration keys and defaults,
the webhook endpoint, log lines and exit codes stay as in 1.3.0, and so do the
files it keeps: `config.toml`, `repos.toml` and `state.json` are read and written
in the same format. Upgrading means replacing the binary, and going back to 1.3.0
is the same swap.

- Same release files for the same seven platforms, plus `SHA256SUMS`, and the same
  Docker image. Linux binaries are static (musl). TLS uses rustls with the system
  certificates and a bundled set as fallback, so nothing besides `git` is needed.
  Distribution builds that rely on the system store can drop the bundled set with
  `cargo build --release --no-default-features`.
- A smaller binary: 3.2 MiB for Linux amd64, against 7.3 MiB for 1.3.0.
- Lower memory and CPU use, see [docs/benchmarks.md](docs/benchmarks.md).
- No async runtime: a few threads with small stacks. A panic while syncing one
  repository is recorded as its error and never stops the daemon.

Fixes:

- Running a command as root (`gitsync sync`, `gitsync run` by hand) no longer
  leaves files the service cannot read. 1.x wrote `state.json` as `root:root`
  with mode 0600, so the next start of the service, which runs as the `gitsync`
  user, failed with "permission denied" and kept failing: nothing was mirrored
  until someone fixed the owner by hand. As root, gitsync now gives new files in
  the state directory (`state.json` and the mirrors under `repos/`) the owner of
  that directory, and a new `repos.toml` the owner of its directory. Existing
  files keep their owner.
- Stopping is fast. SIGTERM, Ctrl+C or a service stop ends the daemon in under 3
  seconds even while a server is not answering: retries and rate limit waits are
  cut short, the state is still saved, and signals are handled while webhooks
  are being set up. Requests already running get the same 60 seconds as before.
- Saving in the `gitsync repos` selector reads `repos.toml` again first, so it no
  longer reverts decisions made since it opened (after a failed refresh, or by
  `gitsync repos allow` from another terminal).
- `gitsync install` also copies the `repos.toml` next to the source config when
  `paths.repos_file` is not set, with the same owner and permissions as the
  config. An existing, different `repos.toml` at the destination is kept, so
  running `install` again to upgrade does not undo decisions made since.
- A crash inside `gitsync repos` leaves the full-screen view before the error is
  printed, so the message stays visible.
- `plan`, `status` and the `failed:` lines of `sync` show OneDev root projects as
  `proj` instead of `//proj`, and `plan` finds their `repos.toml` rule.
- `gitsync --version`, `-v`, `--help` and `-h` work before a command, and
  `gitsync config show --all` works.
- Text prompts in `gitsync repos` keep their label while you type.
- `sync` failures and `plan` removals are listed in a stable order.
- `state.json` is no longer at risk of being torn by two workers saving at once.
  Saves are serialised, flushed to disk, private from the start, and skipped
  when nothing changed.
- Deletions are safer. Rules and filters are checked again right before deleting
  or archiving, so `sync = false`, `include` and `exclude` always win. A mirror
  renamed by hand on GitHub is left alone. An account that suddenly lists no
  repositories is treated as lost access, not as a mass deletion. OneDev errors
  such as a rejected token are no longer read as "repository deleted", and
  per-repository rules now apply to OneDev root projects.
- A private source repository whose GitHub copy was made public by hand is made
  private again before the next push (when `sync.visibility` is on).
- A webhook for a repository waiting for review no longer marks it as excluded.
- A missing repository is logged once instead of on every poll, and retries back
  off up to 5 minutes. `delete_limit` refusals are logged once per hour.
- The poller no longer wakes every second for retries that can never run.
- Descriptions longer than GitHub's limit are cut on a character boundary, so they
  are no longer patched again on every verify.
- A webhook arriving during `gitsync sync` no longer drops the forced push.
- A changed role (`filter.scope = "admin"`) is noticed on the next poll; topics
  listed in a different order no longer cause a rescan.
- `sync.workers = 0` starts one worker instead of none.
- Gitea and Forgejo: a transient error while checking the system webhook no longer
  creates a duplicate, and servers that cap pages below 50 rows are listed in
  full. Gogs: one request less per listing. OneDev: webhooks that are not
  gitsync's are kept exactly as they were. Bitbucket Server: a broken page cursor
  ends the listing instead of looping. Bitbucket Cloud: the token is never sent
  to another host through a `next` link.
- Credentials, including GitLab's `PRIVATE-TOKEN`, are only sent while redirects
  stay on the same host or a subdomain of it.
- Notifications use the configured HTTP proxy like every other request.
- The webhook listener enforces size and time limits, answers `503` when busy and
  keeps serving if one request fails.
- `/dev/null` is no longer taken for a terminal, and `gitsync install` on Windows
  writes characters outside the BMP correctly.

Changes you may notice:

- Configuration syntax errors read `file: line L, column C (last key "x"): ...`
  and never echo the offending line, which could hold a token.
- A token or secret containing control characters is refused, and values from
  `GITSYNC_*` variables are trimmed like those read from files.
- `plan --json` prints `[]` for an empty plan instead of `null`, and JSON output
  no longer escapes `<`, `>` and `&`.
- Dry-run logs show request bodies as JSON.
- Notification errors show only the scheme and host of the URL. Failed
  notifications are retried up to three times.
- An empty `listen.host` also listens on IPv6 where the system supports dual
  stack.

## 1.3.0 (2026-10-01)

- Per-repository `prune` option: `false` keeps branches that exist only on GitHub, so
  pull requests opened from a branch of the repository itself are not closed by the
  next sync.

## 1.2.1 (2026-10-01)

- `gitsync config set`, `config profile` and `repos` now keep the owner and group of
  the file they rewrite. Run as root against a config owned by another user (the
  usual systemd setup), they used to leave a file the service could not read.

## 1.2.0 (2026-10-01)

Choosing what to sync, and a better way to do it. Existing configs keep working
unchanged.

- `gitsync repos`: a full-screen selector for repositories and settings, with
  search, views, per-repository options and profiles. In a pipe or script it prints
  a list, and `allow`, `ignore`, `reset`, `set` do the same from the command line.
- `filter.new_repos` (`sync`, `review`, `ignore`): repositories added later can
  wait for your decision instead of syncing. Repositories already synced are never
  affected.
- `filter.scope = "admin"`: leave alone repositories where you are only a
  collaborator.
- `repos.toml`: per-repository decisions and options (GitHub name, keep private,
  tags, metadata, what to do on removal).
- Profiles `personal`, `team` and `careful`, asked by `gitsync init` and applied
  with `gitsync config profile`.
- `gitsync why`, `gitsync plan`, `gitsync report` and `gitsync config
  show/get/set/edit`; `set` keeps comments and refuses anything that would make
  the file invalid.
- The daemon reloads `config.toml` and `repos.toml` when they change, with no
  signal and no extra wake-ups.
- `[notify]`: a message to a webhook (JSON, plain text, Slack, Discord, ntfy) for
  new repositories, repeated failures and pull requests on mirrors.
- `[pull_requests]`: optionally leave one note on pull requests opened on a
  mirror. They are never merged or closed.
- New dependency: `golang.org/x/term`, used only by the selector.

## 1.0.1 (2026-09-27)

- Each release now also publishes a multi-arch Docker image
  (`linux/amd64`, `linux/arm64`) to the GitHub Container Registry.

## 1.0.0 (2026-09-25)

First release.

- Mirror repositories from Forgejo, Codeberg, Gitea, GitLab, Gogs, GitBucket, OneDev
  and Bitbucket Cloud to GitHub. Bitbucket Server and Data Center are experimental.
- Webhooks for pushes plus a poll that notices renames, visibility, description,
  archiving and deletions.
- Binaries for Linux, macOS and Windows, a Docker image recipe, a systemd service
  and a Windows scheduled task.
