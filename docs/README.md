# Documentation

| Page | What it covers |
|---|---|
| [Configuration](configuration.md) | every key of `config.toml` and `repos.toml`, with its type, default and meaning; where the files are; secrets and environment variables |
| [Servers](providers.md) | tokens, owners, webhook settings and API limits for Forgejo, Codeberg, Gitea, GitLab, Gogs, GitBucket, OneDev and Bitbucket |
| [Choosing what to sync](choosing-repos.md) | rules, decisions and options per repository, profiles, review of new repositories, pull requests on mirrors, notifications |
| [How it works](architecture.md) | modules, threads, the poll, one sync step by step, failures, deletions, credentials, state |
| [Benchmarks](benchmarks.md) | push latency and resources, every change on every server, 2.0 against 1.3 |
| [Troubleshooting](troubleshooting.md) | file ownership after running as root, dubious ownership, webhooks, GitHub token scopes, common messages |
| [FAQ](faq.md) | what is copied, two-way sync, pull requests, visibility, deletions, renames, load on the server |

Outside this folder: the [README](../README.md) for installing and a first run,
[CHANGELOG.md](../CHANGELOG.md), [CONTRIBUTING.md](../CONTRIBUTING.md) and
[SECURITY.md](../SECURITY.md).

[`logo.svg`](images/logo.svg) is drawn by hand and is an input. The other images in
[images/](images) are generated: `build.py` writes the diagram and the social
preview, and `capture.py` records the terminal captures by running a release
binary against fake local servers (`python3 docs/images/capture.py target/release/gitsync`).
