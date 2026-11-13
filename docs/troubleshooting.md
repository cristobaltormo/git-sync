# Troubleshooting

Start with `gitsync check`. It reads the config, logs in to both sides, checks the
GitHub token's scopes, looks up every account on both sides and prints the
webhook URL the git server will be given. Most setup problems show up there.

After that:

- `gitsync status` shows the last error of each repository.
- `gitsync why <repo>` explains why a repository is or is not synced.
- `log.level = "debug"` logs every webhook received. Tokens are replaced by `***`
  in every log line, but read a log before pasting it anywhere.
- The service log is `journalctl -u gitsync -f` on Linux,
  `C:\ProgramData\gitsync\gitsync.log` on Windows, and `docker logs gitsync` in a
  container.

## The service cannot read state.json

The service starts, fails with `permission denied` on `state.json` or on a mirror
under `repos/`, and keeps failing.

This happens on installs made with `gitsync install` when a command was run by hand
as root with gitsync 1.x (`sudo gitsync sync`, or `sudo gitsync run` to test
something). 1.x wrote `state.json` as `root:root` with mode 0600, and the service,
which runs as the `gitsync` user, cannot read it afterwards. 2.0 no longer does
this: as root, it gives new files in the state directory the owner of that
directory. Files written earlier keep their owner, so fix them once:

```sh
sudo systemctl stop gitsync
sudo chown -R gitsync:gitsync /var/lib/gitsync
sudo systemctl start gitsync
```

To run a command by hand against the service's files, run it as the service user:

```sh
sudo -u gitsync gitsync status -c /etc/gitsync/config.toml
```

## git says "detected dubious ownership"

git refuses to work in a repository owned by another user. A message such as
`detected dubious ownership in repository at '/var/lib/gitsync/repos/...'` means a
mirror belongs to a different user from the one running gitsync, usually after the
state directory was copied, restored from a backup or created by root.

Give the state directory back to the user that runs gitsync, as in the previous
section, or delete the affected mirrors: they are a cache and are fetched again on
the next sync. Running commands as the service user, as shown above, avoids it.

## Webhooks do not arrive

Symptoms: a push takes 4 to 8 seconds instead of 2 to 5, and with
`log.level = "debug"` no `webhook for owner/name` line appears when you push.

1. Check the address. `gitsync check` prints the webhook URL. It must be an
   address the git server can reach: `127.0.0.1` only works when both run on the
   same machine, and in Docker it has to be the container's name or the host's
   address, set in `listen.public_url`.
2. Check the path. From the machine of the git server, ask the listener for its
   health endpoint, replacing the address with yours:

   ```sh
   curl http://192.0.2.10:9001/health
   ```

   `ok` means it is reachable. If it is not, the listener may be bound to
   `127.0.0.1` (set `listen.host = "0.0.0.0"`) or a firewall is in the way.
3. Allow the target on the git server. Most servers refuse to call private
   addresses or the machine itself unless told otherwise; the setting for each one
   is in [providers.md](providers.md). Forgejo and Gitea show each delivery and its
   error in the webhook's settings page.
4. Check the signature. A line `webhook rejected: bad or missing signature` means
   the request arrived but the secret does not match. This happens after changing
   `listen.secret`. Run `gitsync hooks --force` to rewrite the webhooks with the
   current secret and URL.

If you would rather not change the git server's settings, `hooks.mode = "none"`
leaves it alone and the poll does the work.

## GitHub token scopes

`gitsync check` lists the problems it finds with the GitHub token:

- `GitHub token has no 'repo' scope`: a classic token needs `repo` to create and
  push repositories, private ones included.
- `no 'delete_repo' scope: deleting mirrors will fail`: only matters with
  `on_delete = "delete"`. Add the scope, or set `on_delete = "archive"` or
  `"ignore"`.
- `fine-grained token: it needs Administration and Contents write on the target
  repos`: GitHub does not report the permissions of fine-grained tokens, so check
  them on the token's page. Administration covers creating, renaming and changing
  settings; Contents covers pushing.
- `the token belongs to 'x' and cannot create repos under the user 'y'`: a token
  can only create repositories in its own account or in organizations it belongs
  to. Map that source owner to the token's own login or to an organization.

A repository you deleted at the source and still see on GitHub usually means the
`delete_repo` scope is missing. The log has the error, and `gitsync status` shows
it next to the repository.

## Other messages

- `already exists on GitHub and gitsync did not create it`: gitsync does not
  overwrite repositories it did not create. Rename one of them, or set
  `sync.adopt_existing = true` to take it over (its content is replaced by the
  source).
- `a previous visibility change is still in progress`: GitHub is still applying a
  visibility change. gitsync waits ten seconds and tries again.
- A push rejected because it contains a secret: GitHub's push protection found a
  token in the history. The repository is retried with growing delays until the
  secret is removed from the source history.
- `unknown option(s) in config.toml`: a key is misspelt or in the wrong section.
  [configuration.md](configuration.md) lists every key.
- `config file not found`: run `gitsync init`, or point to the file with `-c` or
  `GITSYNC_CONFIG`.

## Asking for help

Open a [discussion](https://github.com/cristobaltormo/git-sync/discussions) for
questions, or an issue for a bug, with the output of `gitsync version` and
`gitsync check` and the relevant part of a debug log.
