# Show available recipes
default:
    @just --list

# The justfile is the single source of truth for CI: the workflow runs
# `just ci-check`. Add a check to `verify` and both local and CI runs get it.
# Conformance is deliberately NOT in `verify` (see SPEC.md, "Conformance").

# Check formatting without modifying files
fmt-check:
    cargo fmt -- --check

# Format code
fmt:
    cargo fmt

# Lint every target and feature, warnings denied (levels live in Cargo.toml)
clippy:
    cargo clippy --all-targets --all-features -- -D warnings

# Unit, integration and doc tests (the conformance target is excluded by test = false)
test:
    cargo test --all-features

# Scan dependencies for known vulnerabilities
audit:
    cargo audit

# Tests for the delegation tooling (tools/krs.py; Python 3.11+, stdlib only)
tools-test:
    python3 -B tools/test_krs.py

# Every verification step, in CI order
verify: clippy audit test tools-test

# Format in place, then run every check
ci: fmt verify

# What GitHub Actions runs: unformatted code fails instead of being rewritten
ci-check: fmt-check verify

# Run the upstream cases and rewrite conformance/STATUS.md (never fails)
conformance:
    cargo test --test conformance

# Fail if a case in conformance/baseline.txt no longer passes (the ratchet)
regressions:
    cargo test --test conformance -- --check-regressions --no-status

# Record today's passing cases as the new baseline (after a reviewed improvement)
baseline:
    cargo test --test conformance -- --write-baseline

# Run matching upstream cases with diffs, e.g. `just case namereference`
case filter:
    cargo test --test conformance -- {{filter}} --verbose

# Re-record fixtures and field-spec tables from kustomize (needs go): `just import v5.8.0`
import ref="master":
    scripts/import-upstream.sh {{ref}}

# Delegation tool: `just krs status`, `just krs next`, `just krs brief <task>` ... (see delegation/README.md)
krs *args:
    python3 tools/krs.py {{args}}

# Try the CLI against a directory: `just build path/to/overlay`
build dir=".":
    cargo run -q --features cli -- build {{dir}}
