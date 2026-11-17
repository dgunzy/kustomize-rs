# krusty fixtures

Data recorded from upstream kustomize's end-to-end test suite
(`api/krusty/*_test.go`). Every `Harness.Run` / `RunWithErr` call in those
tests became one directory here:

```
<test file stem>/<TestName>[/<subtest>]/<NN>/
  case.yaml            how to run it (see below)
  input/               the in-memory filesystem the test built, rooted at /
  expected.yaml        the build output (when expect: output)
  expected-error.txt   upstream's error message (when expect: error)
```

`NN` numbers the runs within one test: some tests build, change a file, and build
again.

## case.yaml

```yaml
upstream:
  file: "api/krusty/simple_test.go"   # where the test lives upstream
  test: "TestSimple1"                 # Go test name (subtests use /)
run:
  path: "."                # directory to build, relative to input/
  loadRestrictions: rootOnly   # rootOnly | none  (Flux uses none)
  reorder: none            # none | legacy | unspecified
  addManagedByLabel: false
  plugins: disabled        # enabled = needs exec/Go/fn plugins or helm
expect: output             # output | error
filesystem: inMemory       # onDisk = input/ was not captured
asserted: true             # expected.yaml is the test's golden string
tweaked: true              # (rare) upstream post-processed the output first
```

When `asserted` is absent, upstream checked that run some other way, and
`expected.yaml` is simply what kustomize produced.

Do not edit these files by hand. Regenerate them with
`scripts/import-upstream.sh <ref>`. `UPSTREAM_COMMIT` records the kustomize
commit they came from.

Known recording artifacts at the current commit:

- `fnplugin/*` cases recorded `expect: error` because upstream needs Docker to
  run container functions. They're plugin cases and out of scope anyway.
- `managedbylabel` expects a `kustomize-<version>` label value that depends on
  how the upstream binary was built.

## License and provenance

Derived from [kustomize](https://github.com/kubernetes-sigs/kustomize),
Copyright The Kubernetes Authors, licensed under the Apache License 2.0. The
full text is in `LICENSE.kustomize`.

**Modifications:** the inputs and golden outputs were written as Go string
literals inside upstream's test functions. Here they're recorded as plain files
by running those tests with an added recorder (`scripts/upstream/`). For runs
that upstream didn't check against a golden string, `expected.yaml` is
kustomize's actual output. `case.yaml` is new metadata written by the recorder.
