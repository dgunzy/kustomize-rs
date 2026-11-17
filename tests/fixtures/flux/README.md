# Flux fixtures

`testdata/` is copied verbatim from
[`fluxcd/pkg/kustomize/testdata`](https://github.com/fluxcd/pkg/tree/main/kustomize/testdata)
at the commit in `UPSTREAM_COMMIT`. It backs the tests for Flux's kustomization
generator and post-build variable substitution (`kustomize_generator_test.go`,
`kustomize_varsub_test.go` upstream).

Unlike the krusty fixtures, these are **raw inputs, not yet cases**. The Go tests
that drive them build `Kustomization` objects inline. Turning each one into a
`case.yaml` is part of the Flux layer work (SPEC.md, phase F1).

Copyright The Flux authors, Apache License 2.0. See `LICENSE.fluxcd-pkg`.
