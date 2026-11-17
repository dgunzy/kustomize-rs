# kustomize-rs

A Rust implementation of the [kustomize](https://github.com/kubernetes-sigs/kustomize)
build engine, scoped to the features [Flux](https://fluxcd.io) uses. It's a
library first: `kustomize::build(fs, dir, options)` returns the resources
`kustomize build` would.

**Status: early.** Scaffolding and plain resource and base composition work.
Everything else returns a clear "not implemented yet" error rather than wrong
output. Progress against upstream kustomize's own test cases is tracked in
[`conformance/STATUS.md`](conformance/STATUS.md).

```rust
use kustomize::{build, BuildOptions, fs::DiskFs};

let fs = DiskFs::confined("./repo")?;
let resources = build(&fs, "./repo/apps/prod", &BuildOptions::flux())?;
print!("{}", resources.to_yaml()?);
```

See [SPEC.md](SPEC.md) for scope, design and the roadmap.

## Development

See [CONTRIBUTING.md](CONTRIBUTING.md) and [AGENTS.md](AGENTS.md).

```bash
just ci            # format, lint, unit tests, fixture-leak check
just conformance   # score against recorded upstream cases (informational)
just regressions   # the ratchet: no previously passing case may fail
just case simple   # run matching cases with diffs
just krs next      # delegated work that's ready (see ROADMAP.md, delegation/)
```

## Acknowledgements

This project ports the build engine of
[kustomize](https://github.com/kubernetes-sigs/kustomize) (Copyright The
Kubernetes Authors) and Flux's build steps from
[fluxcd/pkg](https://github.com/fluxcd/pkg) (Copyright The Flux authors), both
Apache-2.0. Its test fixtures and field-spec tables come from those projects.
[NOTICE](NOTICE) lists exactly what was taken and how it was changed.

Kubernetes, kustomize and Flux are trademarks of The Linux Foundation. This
project is not affiliated with or endorsed by The Linux Foundation, the CNCF,
or the Kubernetes or Flux projects.

## License

Apache-2.0; see [LICENSE](LICENSE) and [NOTICE](NOTICE).
