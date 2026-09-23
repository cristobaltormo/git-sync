# Servers

What gitsync needs from each git server, what to configure so its webhooks get
through, and what each API does not offer. Every setting below was checked
against a real server unless the section says it is experimental.

In all cases `source.url` is the address gitsync itself uses to reach the server,
and `[accounts]` maps a source owner to a GitHub owner.

## Forgejo and Codeberg

Set `type = "forgejo"`, or `type = "codeberg"` for codeberg.org (its URL is the
default).

- **Token:** Settings, Applications. Scopes `read:repository` and
  `write:repository` for the repositories and their webhooks, `read:user` and
  `read:organization` for `gitsync check` and `init`. An admin token (`write:admin`)
  lets gitsync create one system-wide webhook.
- **Owners:** users and organizations.
- **Allowing webhooks to the machine itself:** the server refuses private and
  loopback targets by default. In `app.ini`:

  ```ini
  [webhook]
  ALLOWED_HOST_LIST = loopback
  ```

  Use `private` when gitsync runs on another machine of your network. codeberg.org
  cannot reach a private address at all: expose the listener and set
  `listen.public_url`, or use `hooks.mode = "none"`.
- **Detection:** edits bump `updated_at`, so pushes and metadata changes show up
  in the poll immediately.
- **System webhook:** Forgejo never lists the system webhooks it created. gitsync
  keeps the id in its state file and looks the webhook up by id.

## Gitea

Set `type = "gitea"`. Same token scopes, owners and admin webhook as Forgejo.

- **Allowing webhooks:** `[security] ALLOWED_HOST_LIST = loopback` since Gitea
  1.27. Older versions and Forgejo use `[webhook]`.
- **Webhooks:** Gitea applies a system webhook created through its API only after
  a restart, so with `hooks.mode = "auto"` gitsync creates webhooks per repo, which
  work at once. Set `mode = "system"` if you prefer one webhook for everything, and
  restart Gitea after the first run.
- **Detection:** only pushes and topic changes bump `updated_at`. Visibility,
  description, archiving and renames are noticed because the poll compares the
  mirrored fields of the listing, not the update time.

## GitLab

Set `type = "gitlab"`. The URL defaults to https://gitlab.com.

- **Token:** a personal access token with the `api` scope (it has to create
  webhooks), or `read_api` if you only use `hooks.mode = "none"`. An admin token
  enables one system webhook for the whole instance.
- **Owners:** namespaces: a user name, a group, or a subgroup path such as
  `team/web`. A namespace lists only its own projects, so every subgroup you want
  goes in `[accounts]` on its own line.
- **Visibility:** `internal` projects count as private.
- **Allowing webhooks:** Admin area, Settings, Network, Outbound requests: allow
  requests to the local network from webhooks and integrations.
- **Deletions:** recent GitLab versions delete projects in two steps: the project is
  renamed to `<name>-deletion_scheduled-<id>` and stays listed with
  `marked_for_deletion_at` set. gitsync treats those as already gone, so the GitHub
  copy is deleted (or archived) instead of being renamed. A system webhook notices
  it at once, and the removal is confirmed before anything is touched on GitHub.

## Gogs

Set `type = "gogs"`.

- **Token:** Settings, Applications.
- **Owners:** users and organizations.
- **Webhooks:** Gogs has no system-wide webhooks, so they are created per repo.
  Gogs 0.13 and later refuse local targets:

  ```ini
  [security]
  LOCAL_NETWORK_ALLOWLIST = 127.0.0.1
  ```

- **Git over HTTP:** Gogs wants basic authentication as the owner of the token
  instead of the token scheme the others accept. gitsync looks the user name up
  from the API.
- **Default branch:** Gogs can report `master` for a repo that only has `main`.
  GitHub refuses that, gitsync logs it once and does not try again.

## GitBucket

Set `type = "gitbucket"`.

- **Token:** a personal access token from the account's Applications page.
- **Owners:** users and groups.
- **Webhooks:** created per repo, signed with `X-Hub-Signature-256`.
- **What the API does not have:** GitBucket exposes no update time, no archived
  flag and no topics. Visibility, description, renames and deletions are noticed
  by the poll, but a push is only noticed through its webhook. As a safety net,
  the check that runs every `verify_interval` pushes every repo again.

## OneDev

Set `type = "onedev"`.

- **Token:** an access token of a user who can read the projects and edit their
  settings, since that is where webhooks live.
- **Owners:** OneDev projects form a tree, so the owner of a project is the path
  of its parent, and projects at the root belong to the owner `"/"`:

  ```toml
  [accounts]
  "/" = "my-org"            # projects at the root
  "platform" = "my-org"     # projects directly under "platform"
  ```

- **Visibility:** the API does not say whether a project is public, so every
  project is treated as private and never opened on GitHub.
- **What the API does not have:** no update time, archived flag or topics. As with
  GitBucket, pushes arrive through webhooks and the hourly check is the safety
  net.
- **Webhooks:** per repo, added to the project settings. OneDev sends the secret
  in `X-OneDev-Signature` as it is, and the payload carries only the project id,
  which gitsync resolves with one more request. When OneDev runs in Docker,
  `listen.public_url` must be an address the container can reach, not
  `127.0.0.1`.

## Bitbucket Cloud (experimental)

Set `type = "bitbucket-cloud"`. Written from the API documentation and only
exercised against a simulated server, since testing it needs an Atlassian
account. Expect rough edges and please report them.

- **Token:** a repository, project or workspace access token with the
  `repository` and `webhook` scopes, used as a bearer token.
- **Owners:** workspace slugs.
- **Webhooks:** per repo, signed with `X-Hub-Signature`.

## Bitbucket Server and Data Center (experimental)

Set `type = "bitbucket-server"` and `url` to the server's base URL. Same status
as Bitbucket Cloud.

- **Token:** an HTTP access token with project read and repository admin, since
  webhooks need it.
- **Owners:** project keys, as they appear in the URLs.
- **What the API does not have:** no update time and no default branch in the
  listing, so pushes arrive through webhooks and the hourly check.

## Anything else

Servers that speak the Gitea or GitHub API shapes may work as one of the types
above with small differences. A server that is not listed needs a provider: see
[CONTRIBUTING.md](../CONTRIBUTING.md).
