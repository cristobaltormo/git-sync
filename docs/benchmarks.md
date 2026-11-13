# Benchmarks

Rough measurements, to see where gitsync stands and what it costs to run. One machine,
one home network and a few runs each, so read them as orders of magnitude and not as a
scoreboard.

The figures below were taken with gitsync 1.x, written in Go. The section
[Rust 2.0](#rust-20) compares the rewrite against them.

Everything ran on a mini PC (Ryzen 7 7730U) in Debian 13 containers: each git server,
gitsync and the test scripts on the same host, talking to github.com over a home fibre
connection.

## Push latency and resources

For every contender a script creates a commit on a Gitea 1.27 repo through its API and
asks GitHub's API every half second until the new file shows up, 20 times, three seconds
apart. CPU and memory come from the container's cgroup, so the children (git, curl, jq,
the shell) are counted. The idle figures were taken over 60 seconds with 20 repos on the
source.

| | Median | Fastest to 90th percentile | RAM idle | CPU idle | CPU per push |
|---|---|---|---|---|---|
| gitsync, webhook | 4.0 s | 2.3 to 5.7 s | 11.7 MB | 13 ms/min | 86 ms |
| gitsync, poll only | 7.2 s | 3.4 to 7.5 s | 11.6 MB | 9 ms/min | 68 ms |
| Previous bash script, webhook | 4.0 s | 3.9 to 4.7 s | 20.9 MB | 5 ms/min | 142 ms |
| Gitea's push mirror, sync on commit | 4.1 s | 3.9 to 4.8 s | - | - | - |

How to read it:

- With a webhook, gitsync, the bash script it replaces and Gitea's own push mirror all land
  within a fraction of a second of each other. About 2.5 seconds of that is Gitea taking
  that long to deliver the webhook, and about 1.3 seconds is `git push` to GitHub. That is
  the floor for a webhook-based tool, and gitsync is on it.
- Without webhooks the poll adds what it should: up to 5 seconds.
- gitsync needs half the memory of the script and a bit over half the CPU per push. When
  idle the script costs less CPU, because it only listens and never polls.
- The script lost 2 pushes out of 20: no sync arrived within a minute. gitsync lost none.
- Gitea's push mirror only pushes repos that already exist on GitHub. It does not create
  them or follow visibility, description, topics, renames or deletions.
- Three gitsync samples took between 18 and 20 seconds. In each of them a `git push` to
  GitHub stalled and gitsync cut it after 15 seconds of silence and retried, see below.

## Every change on every server

A script drives each server, through its API and where that has none through its web
interface or plain git, and times how long GitHub takes to show each change. Seconds,
one run per server, default settings (poll every 5 seconds).

| Server | New repo | Push | Branch | Tag | Description | Topics | Visibility | Archive | Rename | Delete |
|---|---|---|---|---|---|---|---|---|---|---|
| Forgejo 16 | 4.4 | 4.2 | 3.0 | 2.9 | 3.1 | 6.2 | 3.2 | 6.2 | 5.7 | 14.4 |
| Gitea 1.27 | 3.1 | 3.0 | 4.8 | 3.1 | 5.1 | 4.8 | 4.9 | 3.8 | 5.7 | 11.8 |
| GitLab 19 | 3.2 | 3.1 | 18.2* | 3.1 | 1.8 | 1.9 | 2.0 | 2.0 | 1.7 | 11.5 |
| Gogs 0.14 | 4.4 | 17.7* | 1.8 | 1.7 | - | - | - | - | - | 11.6 |
| GitBucket 4.48 | 5.5 | 1.7 | 1.8 | 1.8 | 3.2 | - | 4.6 | - | 5.7 | 12.2 |
| OneDev 16 | 3.2 | 1.8 | 1.9 | 1.7 | 1.9 | - | - | - | 5.7 | 11.7 |

A dash means the server has no such operation or does not expose it.

- Webhooks make pushes, branches and tags the fastest column everywhere. Edits that a
  server does not announce (Forgejo, Gitea and the others in the table) are found by the
  poll, so they take up to 5 seconds plus the time to apply them.
- GitLab announces edits, renames and deletions too, through its system webhook, and is
  the fastest across the board.
- Deleting takes about 12 seconds where the server sends nothing: up to 5 for the poll and
  10 of deliberate wait before anything is deleted on GitHub.
- Cells marked * include a `git push` that stalled and was retried.

## About the stalls

While measuring, some `git push` runs to GitHub sat for about 95 seconds in the TLS
handshake of git's libcurl (GnuTLS) before failing. It happened to plain `git ls-remote`
too, and it was much worse while the test scripts were polling GitHub's API several times
a second, which got the token rate limited as well. gitsync now runs git with
`--progress` and treats a command that has written nothing for 15 seconds as a stalled
connection: it kills it and tries again, up to three times. A large transfer keeps
reporting progress, so it is never cut. That turned the 95-second pauses into 15 to 20
seconds. Measuring latency against GitHub with aggressive polling gives worse numbers than
real use, which is why the scripts here poll every half second or slower.

## Rust 2.0

gitsync 2.0 is a rewrite in Rust with the same behaviour. The tables above stay as
the reference; this section measures 2.0 against 1.3.0 on the same machine.

Both versions ran one after the other on the same mini PC, in a Debian 13 container,
against the same Forgejo source (three accounts, 32 repos, 30 of them synced and two
ignored) and the same GitHub target. Each binary had its own copy of the same
`config.toml`, `repos.toml` and `state.json`, with `sync.dry_run = true`,
`hooks.mode = "none"`, a 5 second poll and two workers, so they read the same data and
did the same work without writing anything. Each daemon was given 20 seconds to settle
and then sampled for 120 seconds. 1.3.0 is the release binary; 2.0 is the static musl
build.

| | 1.3.0 (Go) | 2.0 (Rust) |
|---|---|---|
| Binary size, Linux amd64 | 7.3 MiB | 3.2 MiB |
| Memory, steady (VmRSS) | 11.8 MiB | 4.3 MiB |
| Memory, peak incl. the startup burst of 30 jobs (VmHWM) | 11.9 MiB | 4.5 MiB |
| Threads | 6 to 7 | 5 |
| CPU while polling, ms per minute | 26.5 | 15.1 |
| CPU over the first 140 s, startup included | 110 ms | 60 ms |
| `gitsync version`, median of 20 | 4.5 ms | 2.5 ms |
| `gitsync status`, median of 20 | 6.6 ms | 4.1 ms |
| `gitsync repos list`, median of 20 | 159 ms | 154 ms |
| `gitsync plan`, median of 10 | 171 ms | 148 ms |
| Exit after SIGTERM | 7 ms | 3 ms |

How to read it:

- Memory is the clearest change: about a third of what 1.3.0 needed, and the startup
  burst barely moves it.
- Idle CPU is lower too, but both are small. These figures are the gitsync process alone
  (`/proc` schedstat), with three accounts to poll, so they are not comparable with the
  cgroup figures in the first table.
- Commands that only read local files finish about 40 % sooner. `repos list` and
  `plan` spend most of their time waiting on the servers, so the gap there is 5 to
  25 ms.
- Both versions queued the same 6 changed repos and 24 due verifications, their logs
  matched line for line, and a `state.json` written by 2.0 is byte for byte the one
  1.3.0 writes, so upgrading and going back are both a binary swap.
- The runtime figures were taken with a 2.0 build optimised for speed, 4.0 MiB on
  disk. Release builds are now optimised for size and come to 3.2 MiB; that build was
  not measured again.
