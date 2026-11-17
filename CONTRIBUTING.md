# Contributing to kustomize-rs

Thank you for your interest in contributing!

## DCO sign-off

All commits must be signed off with the [Developer Certificate of Origin (DCO)](https://developercertificate.org/). Add `-s` to your commit command:

```sh
git commit -s -m "your commit message"
```

This adds a `Signed-off-by` trailer to the commit. The DCO check fails on PRs without it.

## Development

Read [SPEC.md](SPEC.md) for scope and design, and [AGENTS.md](AGENTS.md) for the
conventions (it's written for AI agents but applies to everyone). The short
version: pick a feature from `conformance/STATUS.md`, read the upstream Go,
write unit tests, implement, and run:

```sh
just ci            # formatting, linting, audit, tests: what CI gates on
just conformance   # updates conformance/STATUS.md: include it in the PR
```

## Upstream code and attribution

kustomize and Flux are Apache-2.0. When you port logic from them, put the
attribution header from AGENTS.md ("Licensing") at the top of the file. Add a
line to `NOTICE` if you bring in a new kind of upstream material.

## AI-assisted contributions

These are welcome. Agents follow AGENTS.md, which includes **no git or GitHub
writes**. You, the human author, review, sign off and commit the change, and
you're responsible for it.

## Submitting a pull request

1. Fork the repo and create a branch from `main`
2. Make your changes with signed-off commits
3. Ensure `just ci` passes locally
4. Open a PR targeting `main`

PRs require one approving review and all CI checks to pass before merging.
Conformance is reported on every PR but never blocks a merge.
