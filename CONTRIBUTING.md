# Contributing

Bug reports and pull requests are welcome. For anything bigger than a fix, open an
issue first so we can agree on the approach.

## Building and testing

```
make test      # go vet and go test -race
make build
```

Go 1.24 or newer. The tests use fake servers and need no network. A change is not
done until `make test` passes.

## Adding a git server

A server is a type in `internal/forge` that implements `forge.Provider`:

- `List(owner)` and `Get(owner, name)` map the server's repos to `forge.Repo`. Leave
  a field empty when the API has no equivalent (`Updated`, `DefaultBranch`), the
  engine copes with that.
- `CloneURL` and `GitHeader` say how git reaches the repo over HTTP.
- `ParseWebhook` verifies the signature or token of an incoming webhook and returns
  the repo it is about, or `nil` for events that do not matter.
- `Hooks` creates and removes webhooks. Return `nil` if the server has none.

Register the type in `forge.New` and in `config.SourceTypes`, add a test file with a
fake server that answers the way the real one does, and describe the server in
`docs/providers.md`. Say in the pull request how you tested it against a real
instance; a provider that was not is marked experimental.

## Style

Small functions, no comments that repeat the code, a comment where a reader could not
guess why. Commit messages start with a short imperative summary.
