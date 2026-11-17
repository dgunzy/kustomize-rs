# Upstream unit-test tables

Table-driven test cases from kustomize's filter tests, extracted as data by
`scripts/upstream/extract_filter_tables.py` (run by `scripts/import-upstream.sh`).
Rust unit tests read them with `include_str!` and run exactly upstream's cases.
Results are compared as data, not as text.

| File | Upstream |
|---|---|
| `fieldspec_filter.json` | `api/filters/fieldspec/fieldspec_test.go` |
| `merge2.json` | `kyaml/yaml/merge2/{element,list,map,scalar}_test.go` |
| `<filter>_filter.json` | `api/filters/<filter>/*_test.go` (generic: YAML fields as data, the filter settings as Go text under `go`) |

Copyright The Kubernetes Authors, licensed under Apache-2.0 (see
`../krusty/LICENSE.kustomize`). **Modified:** converted from Go test code into
JSON. Cases whose filter the extractor can't read are marked `unsupported`.
Don't edit by hand.
