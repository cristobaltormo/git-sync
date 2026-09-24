# Security

gitsync holds two access tokens that can write to your repositories, so a problem
in it matters. Please report vulnerabilities privately through GitHub's security
advisories for this repository instead of opening a public issue. You can expect
an answer within a few days.

Things worth knowing when you run it:

- Keep the config file readable only by the user that runs gitsync. `gitsync init`
  and `gitsync install` set the permissions for you.
- Webhooks are only accepted when they carry a valid signature or secret. Bind the
  listener to localhost unless the git server has to reach it from elsewhere.
- Tokens are passed to git through its environment, never on a command line, and
  are removed from log output.
- Prefer a token limited to what it needs: `repo` on GitHub, and `delete_repo` only
  if you want mirrors deleted.
