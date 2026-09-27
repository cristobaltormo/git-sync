# Changelog

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
