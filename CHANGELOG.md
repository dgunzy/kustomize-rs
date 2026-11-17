# Changelog

All notable changes to this project are documented here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/).

## [Unreleased]

### Added

- Scaffold:
  - filesystem abstraction (in-memory, on-disk, confined);
  - kustomization loading;
  - resources and bases/overlays;
  - cycle detection;
  - `RootOnly` load restriction;
  - duplicate-id detection.
- Conformance runner over 327 cases recorded from kustomize's `api/krusty` test
  suite, reporting to `conformance/STATUS.md`.
- `kustomize-rs build` CLI behind the `cli` feature.
- Conformance ratchet:
  - `conformance/baseline.txt`;
  - `just regressions` / `just baseline`;
  - `--json` results.
- Fixture-leak check (`tests/fixture_leaks.rs`): engine code may not hardcode
  fixture data.
- Delegation tooling (`tools/krs.py`, `tasks/`, `delegation/`):
  - token-budgeted worker briefs;
  - isolated workspaces;
  - mechanical ACCEPT/REJECT gates;
  - an OpenAI-compatible worker loop.
- ROADMAP.md: hardest-first order of work, lead vs worker.
- Transformer framework:
  - the `Transformer` trait;
  - build stages in upstream order;
  - rename lineage and build state on `Resource`;
  - `ResId` equality as kustomize defines it;
  - cluster scope from the schema;
  - `TransformerConfig` from the builtin tables.
- Field-spec engine (`fieldspec::apply`) and setters with kyaml's typing rules;
  passes upstream's fieldspec filter table.
- Strategic-merge schema (`openapi::Schema`, generated from the Kubernetes
  v1.21.2 schema kustomize embeds).
- Strategic merge patch (`smp::merge`), a port of kyaml merge2: passes 59 of
  upstream's 61 merge2 cases (the other 2 put schemas in YAML comments).
- Upstream unit-test tables extracted as data (`tests/fixtures/units/`), and
  included in worker briefs.
