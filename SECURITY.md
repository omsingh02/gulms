# Security Policy

## Supported versions

Only the latest release receives fixes.

## Reporting a vulnerability

Please **don't open a public issue** for security problems. Report them privately through
[GitHub's private vulnerability reporting](https://github.com/omsingh02/gulms/security/advisories/new).
You'll get a response within a few days.

## What matters here

`gulms` handles a login credential, so these are the areas to look at:

- Your password is sent only to the portal's `/login/token.php` and is never written to disk or logs.
- The resulting token is stored in `config.json` with owner-only permissions (`0600` on Unix).
  Anything that exposes it, or sends it anywhere except your portal, is a vulnerability.
- Downloads and syncs only talk to the portal you configured. The only other network request is
  fetching a pinned, integrity-checked copy of Mermaid from jsDelivr when a PDF contains a flowchart.
- The installer scripts verify a SHA-256 checksum before installing.

Revoking a token: `gulms logout` removes it locally; to invalidate it on the portal, reset it under
*Preferences → Security keys*.
