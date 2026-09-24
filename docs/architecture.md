# How it works

gitsync is one process with three moving parts: a webhook listener, a poller and a
small pool of workers. They share a queue of repos to sync, and everything it
knows is kept in one JSON state file.

```
  git server --- webhook ------> listener --+
       ^                                    |
       |                                    v
       +--- listing every 5 s --- poller --> queue --> workers --> git fetch, git push --> GitHub
                                                            |
                                                            +--> GitHub API (settings)
```

## Packages

| Package | Role |
|---|---|
| `cmd/gitsync` | entry point |
| `internal/cli` | the commands: run, sync, check, status, hooks, init, install |
| `internal/config` | the TOML file, defaults, validation, secrets from files and environment |
| `internal/forge` | one provider per git server behind a single interface |
| `internal/github` | the GitHub API client |
| `internal/mirror` | the git commands that copy branches and tags |
| `internal/engine` | state, queue, the decision of what to do for each repo |
| `internal/webhook` | the HTTP listener, which hands events to the engine |
| `internal/httpx` | HTTP client with retries and rate limit handling |
| `internal/logx` | logging that never prints a secret |

## The poll

Every `poll_interval` seconds the engine lists the repos of each configured owner
and hashes the fields it mirrors, plus the update time where the server has one.
If the hash equals the previous one, nothing happened and nothing else is done.
If it differs, every listed repo is compared with what was stored last time and
only those that changed are queued. Listing pages are hashed as raw bytes first, so
an unchanged page is not even decoded.

The comparison is on fields rather than on the update time because servers
disagree on what bumps it: Forgejo updates it on every edit, Gitea only for
pushes and topics.

## Syncing one repo

A job for one repo does, in this order:

1. Work out the GitHub name and owner. If the repo was renamed at the source,
   rename the GitHub repo; if the new name is taken, stop with an error.
2. Find the GitHub repo. If it does not exist, create it private. If it exists
   and gitsync did not create it, stop unless `adopt_existing` is set.
3. Before pushing: unarchive if needed, and make GitHub private if the source
   became private. Visibility is only ever closed before a push.
4. `git fetch` from the source into a bare mirror, then `git push --prune` to
   GitHub. Only branches and tags are transferred, always forced, so the two
   sides end up identical.
5. After pushing: open visibility if the source is public, copy description,
   website, topics and default branch, archive if the source is archived.
6. Record the result, including a snapshot of the GitHub repo, so the next push
   needs no GitHub API calls unless something changed.

Jobs for the same repo never run at the same time. Events that arrive while one
is running are merged into a single follow-up, and webhooks wait 100 ms so the
push and create events of one `git push` become one job.

## Failures

A failed job is retried with growing delays (30 seconds up to 30 minutes). GitHub
answers a few things in a way that means "try again in a moment", such as a
visibility change still in progress, and those are retried within seconds. A repo
that gitsync refuses to touch, for example because the name is taken on GitHub, is
looked at again every ten minutes but only logged once.

A git command that writes nothing at all for 15 seconds is a stalled connection, since even
a very large transfer keeps reporting progress. gitsync kills the whole process group and
tries again straight away, up to three times. Without it a stuck TLS handshake to GitHub
holds a sync for about a minute and a half before git gives up by itself.

## Deletions

A repo that vanishes from a listing is not deleted on the spot. It has to stay
missing for `delete_grace` seconds, the server has to answer that the repo does
not exist when asked for it directly, and its owner has to still exist, because a
token that lost access looks exactly like a deletion. Then `on_delete` decides.
There is also a limit of `delete_limit` deletions per hour, and nothing is
deleted on GitHub that is owned by an account other than the one mapped.

## Credentials

Tokens never appear in a URL, a command line or a `.git/config`. git receives
them as `http.<url>.extraheader` values in its environment, scoped to the server
they belong to, so the source token is never sent to GitHub or the other way
round. Logs pass through a filter that replaces every known secret.

## State

`state.json` maps each source repo id to what gitsync last did with it: the GitHub
name, the last snapshot of the GitHub repo, the last error and when to retry. The
ids survive renames and transfers. The bare mirrors under `repos/` are a cache and
can be deleted at any time.
