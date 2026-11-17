#!/usr/bin/env bash
# Regenerate the upstream-derived data in this repo from a kustomize checkout:
#
#   tests/fixtures/krusty/   one directory per krusty test run (input tree,
#                            case.yaml, expected.yaml or expected-error.txt)
#   src/builtin_config/      kustomize's default field-spec tables
#   src/schema/              schema-derived tables (namespace scope, ...)
#
# The fixtures are recorded by running upstream's own Go test suite with a small
# recorder hooked into its test harness (scripts/upstream/), so expected output
# is exactly what that kustomize version produces. Requires git and go.
#
# Usage: scripts/import-upstream.sh [kustomize git ref, default: master]
set -euo pipefail

ref="${1:-master}"
repo_root="$(cd "$(dirname "$0")/.." && pwd)"
work="$(mktemp -d "${TMPDIR:-/tmp}/kustomize-rs-import.XXXXXX")"
trap 'rm -rf "$work"' EXIT

echo "cloning kustomize@$ref"
git clone --quiet --depth 1 --branch "$ref" https://github.com/kubernetes-sigs/kustomize.git "$work/kustomize" 2>/dev/null \
  || { git clone --quiet https://github.com/kubernetes-sigs/kustomize.git "$work/kustomize" && git -C "$work/kustomize" checkout --quiet "$ref"; }
commit="$(git -C "$work/kustomize" rev-parse HEAD)"

harness="$work/kustomize/api/testutils/kusttest"
cp "$repo_root/scripts/upstream/record.go" "$harness/record.go"
git -C "$work/kustomize" apply "$repo_root/scripts/upstream/harness.patch"
cp "$repo_root/scripts/upstream/dump_fieldspecs_test.go" \
  "$work/kustomize/api/internal/konfig/builtinpluginconsts/zz_dump_test.go"
cp "$repo_root/scripts/upstream/dump_openapi_test.go" \
  "$work/kustomize/kyaml/openapi/zz_dump_test.go"

out="$work/fixtures"
mkdir -p "$out"
echo "recording krusty tests (upstream failures that need docker/helm/network are expected)"
(cd "$work/kustomize/api" && KRS_RECORD_DIR="$out" go test ./krusty/ -count=1 >"$work/go-test.log" 2>&1) || true
grep -E '^\s*--- FAIL' "$work/go-test.log" || true

fixtures="$repo_root/tests/fixtures/krusty"
find "$fixtures" -mindepth 1 -maxdepth 1 -type d -exec rm -rf {} +
cp -R "$out/." "$fixtures/"
cp "$work/kustomize/LICENSE" "$fixtures/LICENSE.kustomize"
echo "$commit" >"$fixtures/UPSTREAM_COMMIT"

config="$repo_root/src/builtin_config"
rm -f "$config"/*.yaml
(cd "$work/kustomize/api/internal/konfig/builtinpluginconsts" && KRS_DUMP_DIR="$config" go test -run TestDumpForKustomizeRs -count=1 . >/dev/null)

schema="$repo_root/src/schema"
rm -f "$schema"/*.json
(cd "$work/kustomize/kyaml/openapi" && KRS_DUMP_DIR="$schema" go test -run TestDump -count=1 . >/dev/null)

python3 "$repo_root/scripts/upstream/extract_filter_tables.py" "$work/kustomize" "$repo_root/tests/fixtures/units"

echo "imported $(find "$fixtures" -name case.yaml | wc -l | tr -d ' ') cases from kustomize $commit"
echo "next: just conformance   (refreshes conformance/STATUS.md)"
