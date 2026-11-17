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
