# Changelog

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
