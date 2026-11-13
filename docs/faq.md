# FAQ

## What exactly is copied to GitHub?

Branches and tags, force-pushed and pruned so GitHub equals the source. On top of
that, where the git server provides them: description, website, topics, default
branch, visibility and the archived flag. Each of these can be turned off in
`[sync]`. Per repository, `repos.toml` can turn off tags and the metadata
(description, website and topics), keep the copy private, and keep branches that
exist only on GitHub (`prune = false`).

Pull requests, issues, wikis, releases, LFS objects, stars and settings such as
branch protection are not copied. Refs other than branches and tags, such as a
server's own `refs/pull/*`, are not pushed either.

## Can it sync in both directions?

No. The git server is the source of truth and GitHub is a copy. Anything committed
directly on GitHub is overwritten by the next sync. If you need two-way sync, this
is not the tool.

## People open pull requests on the mirror. What happens?

Nothing is merged or closed. With `pull_requests.mode = "comment"` gitsync adds one
note asking the author to send the change to the source, and with `[notify]` it
tells you. `gitsync report` lists the open pull requests on your mirrors.

If a public repository takes pull requests on GitHub, set `prune = false` for it in
`repos.toml`, so branches that exist only on GitHub are not removed, and
`on_delete = "ignore"`, so removing the source never deletes the GitHub repository
with its pull requests and issues. See
[choosing-repos.md](choosing-repos.md#pull-requests-and-repositories-created-on-github).

## Will it make a private repository public?

Never. New repositories are created private, pushed, and only opened if the source
is public. A source that becomes private makes the GitHub copy private before the
next push. `sync.visibility = false`, or `keep_private = true` for one repository,
keeps the GitHub side as it is.

## What does it do to repositories I already have on GitHub?

Nothing, unless you ask. A repository that exists on GitHub and was not created by
gitsync is left alone and reported in the log. `sync.adopt_existing = true` lets
gitsync take it over, which replaces its content with the source's. Repositories
that exist only on GitHub are never touched; `gitsync report` lists them.

## What happens when I delete a repository on the git server?

After `delete_grace` seconds (10 by default) gitsync asks the server for it
directly. Only if the server confirms it is gone, and its owner still exists, does
`on_delete` apply: delete, archive or ignore. At most `delete_limit` repositories
(5 by default) are deleted or archived per hour, and an account that suddenly lists
no repositories is treated as lost access, not as a mass deletion. Nothing owned by
an account other than the mapped one is ever touched.

## What happens when I rename a repository?

The GitHub repository is renamed in place, so stars, watchers and links that GitHub
redirects keep working. If the new name is already taken on GitHub, gitsync stops
and says so instead of overwriting anything.

## Do I need webhooks?

No. With `hooks.mode = "none"` gitsync never touches the git server and the poll
notices everything; a push takes up to `poll_interval` seconds longer. Webhooks are
what make pushes arrive in 2 to 5 seconds.

## How much load does the poll put on my git server?

One listing per account every `poll_interval` seconds. Every page of the listing
is still requested each time; gitsync hashes each page and, when it has not
changed, skips decoding it again, which saves work on gitsync's side, not on the
server's. An idle poll is a handful of small API requests. Raise `poll_interval` if you have many
accounts or a slow server.

## Can I mirror to GitHub Enterprise Server?

Set `github.api_url` and `github.git_url` to your instance. It uses the same API,
but it is not part of the tested setups.

## Can I mirror one source to several GitHub accounts?

Each source owner maps to one GitHub owner in `[accounts]`, and different source
owners can map to different GitHub owners. To send the same repositories to two
places, run two instances with separate configs and state directories.

## Can I keep a different name on GitHub?

Yes, per repository: `gitsync repos set alice/site name=site-mirror`, or in the
selector with `o`. Changing it later renames the GitHub copy.

## Does it work with large repositories?

Yes. Transfers go through `git fetch` into a local bare mirror and `git push`, so
only new objects move after the first sync. A transfer that keeps reporting
progress is never cut, and `sync.git_timeout` (30 minutes by default) is the upper
limit for one git command. LFS objects are not mirrored.

## Where does it keep its data, and can I delete it?

`state.json` and one bare mirror per repository under `paths.state_dir`
(`/var/lib/gitsync` for the service). The mirrors can be deleted at any time and are
fetched again. Deleting `state.json` makes gitsync forget which GitHub repositories
it created, so they would count as not created by gitsync until adopted again: keep
it.

## How do I upgrade, and can I go back?

Replace the binary and restart. 2.0 reads and writes the same files as 1.3.0, and
1.3.0 reads what 2.0 writes, so going back is the same swap. See
[Upgrading from 1.x](../README.md#upgrading-from-1x).

## Why is the GitHub repository a mirror and not developed there?

Because the project lives on the git server. gitsync itself is developed that way:
see [CONTRIBUTING.md](../CONTRIBUTING.md) for how pull requests sent on GitHub are
applied.
