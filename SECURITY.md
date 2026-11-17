# Security Policy

## Supported versions

kustomize-rs is pre-release. Only the latest commit on `main` receives fixes.

## Reporting a vulnerability

Please **do not** open a public issue for security vulnerabilities.

Report privately via [GitHub private vulnerability reporting](https://github.com/dgunzy/kustomize-rs/security/advisories/new)
(Security tab → "Report a vulnerability").

Include what you can: the commit, a description of the issue, and a minimal
input that reproduces it. You can expect an acknowledgement within a week.

## Scope notes

kustomize-rs builds untrusted repositories, so these areas are of particular
interest:

- Reading files outside a `DiskFs::confined` root (path traversal, symlinks)
- Ignoring the `RootOnly` load restriction
- Panics, unbounded recursion or memory use on crafted input
- Any network access or process execution (the engine should do neither)

Dependency advisories are monitored via `cargo audit` and `cargo deny` in CI,
plus Dependabot. The repository also runs OpenSSF Scorecard, CodeQL and
gitleaks.
