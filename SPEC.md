# kustomize-rs specification

A Rust library that does what `kustomize build` does, for the inputs Flux
builds. It is not a replacement for the kustomize CLI.

The first consumer is [flux9s](https://github.com/dgunzy/flux9s). It needs to
preview what a Flux `Kustomization` would change (flux9s#343) without shelling
out to the `flux` binary. That needs a native build: a kustomize build, then
Flux's own build steps, then a server-side dry-run. This crate does the first
two. The dry-run and the diff stay in flux9s.

## 1. Goals and non-goals

**Goals**

- A library: `kustomize::build(fs, dir, options) -> ResMap`, producing the same
  resources as kustomize for the same input.
- Cover the kustomize features Flux users rely on first (section 4, tier 1), then
  widen.
- Optionally a Flux layer (section 8): Flux's generated kustomization and
  post-build substitution, so `flux build kustomization` can be reproduced.
- Measure compatibility against upstream's own test cases (section 6), and say
  in the README how much of it passes.
- Safe on untrusted repositories:
  - no panics (`unwrap`/`expect`/`panic` are denied outside tests);
  - no reads outside a confined root;
  - no network or process execution unless a future feature explicitly opts in.

**Non-goals** (now; section 4 keeps them listed so they can be picked up later)

- The CLI's editing commands (`kustomize edit`, `create`, `localize`, `cfg`).
- Exec, Go and KRM-function plugins. Flux builds with plugins disabled.
- Helm chart inflation (`helmCharts`). Flux has HelmRelease for that.
- Byte-identical output (section 7).
- Matching upstream's error message text.

A small `kustomize-rs build <dir>` binary (feature `cli`) exists for trying the
engine on real repositories. It stays a thin wrapper over the library.

## 2. How Flux calls kustomize

Read from `fluxcd/pkg/kustomize` (`kustomize_generator.go`, `Build` and
`SecureBuild`). These settings are the contract for `BuildOptions::flux()`:

| Setting | Flux | kustomize CLI default |
|---|---|---|
| Load restrictions | `LoadRestrictionsNone`, with the filesystem itself confined to the source root (`MakeFsOnDiskSecure`) | `RootOnly` |
| Plugins | disabled (builtins only) | disabled |
| Reorder | unset → **input order**, unless the kustomization sets `sortOptions` | `legacy` |
| Managed-by label | off | off |
| OpenAPI schema | reset per build, builtin schema plus `openapi.path` | same |
| Remote bases | allowed only when the controller runs with `--no-remote-bases=false` | allowed |

Before the build, Flux generates or rewrites `kustomization.yaml` from the
`Kustomization` object's spec (section 8). After it, Flux runs `postBuild`
substitution on every resource.

## 3. Architecture

```
src/
  lib.rs            public API: build, BuildOptions, ResMap, Resource, ResId
  build.rs          the pipeline (kustomize's KustTarget + Kustomizer.Run)
  fs.rs             FileSystem trait; MemoryFs (tests, overlays); DiskFs (optionally confined)
  kustomization.rs  kustomization.yaml: typed fields + `rest` for not-yet-implemented
  resource.rs       Resource, ResId, Gvk, ResMap
  yaml.rs           the only place YAML is parsed or rendered
  builtin_config/   kustomize's default field-spec tables, as YAML (generated)
  bin/              the optional CLI
```

Modules to add as features land (names are a guide, not a contract):

```
  fieldspec.rs      FieldSpec + path walking ("spec/template/metadata/labels", create)
  transformers/     one file per builtin: namespace, prefix_suffix, labels, annotations,
                    images, replicas, patch_smp, patch_json6902, replacements, sort
  generators/       configmap, secret, hash suffix
  nameref.rs        back-reference fixing after renames
  openapi/          merge keys + patch strategies for SMP (generated table + custom schemas)
  flux/             feature "flux": generator, varsub, sourceignore
```

### Data model

- **Resource**: a `serde_json::Map` (insertion-ordered via `preserve_order`),
  plus build-time state kept in Rust fields, not in annotations:
  - original id and previous names, for name references;
  - generator behavior (`create`/`merge`/`replace`);
  - whether a hash suffix is needed;
  - origin, for `buildMetadata`.

  Kustomize stores these as `internal.config.kubernetes.io/*` annotations and
  strips them at the end. Struct fields can't leak into the output.
- **ResId**: group, version, kind, namespace, name. Equality rules are part of
  name-reference work, for example whether `""` and `default` match. The tests
  decide them.
- **ResMap**: ordered `Vec<Resource>`, with duplicate-id rejection on append. It
  stays a Vec because order is part of the output, and N is small.

### Pipeline

This mirrors upstream's order exactly
(`api/internal/target/kusttarget.go`, `api/krusty/kustomizer.go`). Each step
fails with `Unsupported` until it is implemented.

```
build(dir):
  load kustomization file (one of kustomization.yaml | .yml | Kustomization)
  openapi: builtin schema + kustomization.openapi.path
  accumulate(dir):
    1. resources      files (multi-doc, *List flattened) and directories (recurse);
                      remote URLs later
    2. configurations + crds  → merge into the transformer config (field specs)
    3. generators     configMapGenerator, secretGenerator (helmCharts deferred),
                      then `generators:` entries
    4. components     each component accumulates on top of the current resources
    5. transformers   PatchStrategicMerge (patchesStrategicMerge), Patch (patches),
                      Namespace, Prefix, Suffix, Label (commonLabels + labels),
                      Annotations, PatchJson6902, ReplicaCount, ImageTag,
                      Replacement; then `transformers:` entries
    6. validators     (`validators:` entries)
    7. vars           merge (deprecated `vars`)
  after the top-level accumulate only:
    8. hash suffixes  on generated ConfigMaps/Secrets
    9. name refs      fix every reference to a renamed resource
   10. vars           resolve $(VAR) references
   11. drop          resources annotated config.kubernetes.io/local-config
  sort:  sortOptions from the kustomization, else options.reorder
  managed-by label (option or buildMetadata: [managedByLabel])
  strip build annotations; keep origin/transformer annotations only if buildMetadata asks
```

### Errors

- `Error::Unsupported(feature)`: the input needs something not implemented.
  The conformance report groups cases by this string, so it doubles as the
  backlog. Never guess a partial result instead.
- `Error::Invalid`: input kustomize would reject.
- `Error::Io` and `Error::Parse`: carry the path.

Error text doesn't have to match upstream. A case that expects an error passes
on any `Invalid`, `Io` or `Parse` error, but not on `Unsupported`.

## 4. Feature scope

Tiers set the order of work, not a promise. Everything stays open-ended:
"deferred" means not now. The fixture groups are directories under
`tests/fixtures/krusty/`.

### Tier 1: what Flux repositories use most

| Feature | Upstream source | Fixture groups |
|---|---|---|
| `resources`: files, dirs, multi-doc, `*List` | `accumulator`, `resource/factory.go` | simple, basic_io, no_list_items, directoryarrangement |
| bases/overlays, diamonds, cycles | `target/kusttarget.go` | baseandoverlay*, diamond*, repeatbase, complexcomposition |
| `namespace` | `filters/namespace` | namespaces, namespacepropagation, blankvalues |
| `namePrefix` / `nameSuffix` | `filters/prefix`, `filters/suffix` | basereusenameprefix, nameprefixsuffixpatch, disablenamesuffix |
| `commonLabels`, `labels` (incl. `includeSelectors`, `includeTemplates`) | `filters/labels`, `builtins/LabelTransformer.go` | inlinelabels, numericcommonlabels |
| `commonAnnotations` | `filters/annotations` | |
| `images` | `filters/imagetag` | transformersimage |
| `replicas` | `filters/replicacount` | |
| `patches` (SMP and JSON 6902, inline or file, with `target` selectors) | `builtins/PatchTransformer.go` | multiplepatch, inlinepatch, extendedpatch, gvknpatch, patchdelete |
| `patchesStrategicMerge`, `patchesJson6902` (deprecated, still common) | `filters/patchstrategicmerge`, `filters/patchjson6902` | multiplepatch, mergeenvfrom |
| `configMapGenerator` / `secretGenerator` + `generatorOptions` + hash suffix | `builtins/*Generator.go`, `api/hasher`, `api/kv` | configmaps, generatoroptions, generatormergeandreplace, namespacedgenerators |
| name references (renames propagate to referrers) | `filters/nameref` | namereference, nameupdateinroleref, rolebindingacrossnamespace, namedspacedserviceaccounts |
| `components` | `target/kusttarget.go` | component |

### Tier 2: in use, less common

| Feature | Upstream source | Fixture groups |
|---|---|---|
| `replacements` | `filters/replacement` | replacementtransformer |
| `sortOptions` + legacy order | `builtins/SortOrderTransformer.go` | sortordertransformer, legacy_order |
| `configurations` (custom field specs) | `builtinconfig` | customconfig* |
| `crds` | `accumulator/loadconfigfromcrds.go` | crd |
| `openapi` (custom schemas for SMP on CRDs) | `kyaml/openapi` | openapicustomschema |
| `transformers` / `generators` / `validators` with builtin kinds (inline or file) | `target/kusttarget_configplugin.go` | inlinetransformer, transformersarrays, customconfigofbuiltinplugin |
| `buildMetadata` (origin, transformer annotations, managed-by) | `resource/origin.go` | originannotation, transformerannotation, kustomizationmetadata |
| `vars` (deprecated) | `filters/refvar`, `accumulator` | variableref |
| `config.kubernetes.io/local-config` dropping | `kusttarget.go` IgnoreLocal | localconfig |

### Deferred (open: may be implemented later)

| Feature | Why deferred |
|---|---|
| Remote resources (git URLs, `github.com/...`, HTTP) | needs network and git; Flux can disable them. Design note: take a resolver trait so callers supply fetched trees |
| `helmCharts` / HelmChartInflationGenerator | runs `helm`; Flux uses HelmRelease |
| Exec / Go / KRM-function plugins | Flux disables them |
| `iampolicygenerator` | cloud-specific, rarely used with Flux |
| `kustomize edit` / `localize` / `cfg` | CLI features, not build |
| Byte-identical output | section 7 |

## 5. Work plan: TDD, piece by piece

The order of work and who does it (lead vs worker) is in ROADMAP.md: hardest
foundations first, done by the lead, then translation tasks delegated to workers
through `tools/krs.py` (delegation/README.md). The milestones below are the
outcome view of the same plan.

The loop for every feature:

1. Pick the feature with the most cases waiting on it (top of
   `conformance/STATUS.md`), or the next tier-1 row.
2. `just case <fixture-group>` shows those cases with diffs.
3. Read the upstream implementation (`Upstream source` column). Write focused
   unit tests in the module for the behavior and edge cases you find there.
4. Implement it: move the field from `Kustomization::rest` to a typed field,
   then add the transformer or generator.
5. `just ci`, `just regressions`, then `just conformance`. Commit the updated
   `STATUS.md` with the change, so the diff shows which cases it unlocked. Once
   it's reviewed, `just baseline` ratchets the new passes.

A feature is done when its fixture groups pass, or every remaining failure is
understood and written down in STATUS.md or an issue.

### Milestones

| # | Milestone | Done when |
|---|---|---|
| M0 | Scaffold: fs, kustomization loading, resources, bases, conformance runner | **done** (22 cases pass) |
| M1 | Field specs + namespace, prefix/suffix, labels, annotations, images, replicas | their fixture groups pass; builtin_config tables loaded |
| M2 | JSON 6902 patches; `patches` target selectors | gvknpatch, inlinepatch (6902 cases) |
| M3 | Strategic merge patch with the builtin OpenAPI table | multiplepatch, mergeenvfrom, patchdelete |
| M4 | Generators + hash suffix + name references | configmaps, generatoroptions, namereference |
| M5 | Components | component, complexcomposition |
| M6 | Flux layer F1 (generator) and F2 (varsub) | Flux fixtures pass; flux9s can build a real repo |
| M7+ | Tier 2 in STATUS.md order | open-ended |

M6 is the point where flux9s can start using it, behind its preview flag.

### Notes for the hard parts

- **Field specs.** Paths like `spec/template/metadata/labels` walk through
  arrays implicitly. `create: true` creates missing maps. `[]` and escaped `\/`
  appear in paths. The defaults are `src/builtin_config/*.yaml`, generated, not
  hand-copied. Port `filters/fieldspec` and `filters/fsslice` faithfully: every
  transformer depends on them.
- **Strategic merge patch.** It needs `x-kubernetes-patch-merge-key` and
  `x-kubernetes-patch-strategy` per field. Kustomize embeds the full Kubernetes
  v1.21.2 schema (1.4 MB protobuf). Generate a compact table instead
  (GVK → path → merge key, strategy, and list type) with a script, checked in
  as data. Custom `openapi:` schemas extend it at runtime. Directives to support:
  - `$patch: delete | replace | merge`
  - `$retainKeys`
  - `$setElementOrder`
  - `$deleteFromPrimitiveList`
  - null deletes a field
- **Hash suffix** (`api/hasher`):
  1. JSON-encode the inputs with sorted keys:
     - ConfigMap: `{kind, name, data, binaryData?}`;
     - Secret: `{kind, type, name, data, stringData?}`.
  2. Take the sha256 hex.
  3. Keep the first 10 characters, mapping `0→g`, `1→h`, `3→k`, `a→m`, `e→t`.
- **Name references.** Port `filters/nameref` using the `namereference.yaml`
  table. A referrer is updated only when the candidate is unambiguous, which
  depends on namespace and on prefix/suffix lineage. The namereference,
  rolebindingacrossnamespace and namespaces groups cover the edge cases.
- **Metadata strings.** Kustomize writes label and annotation values as strings
  (`happy: true` becomes `"true"`) and emits them with sorted keys. `basic_io`
  covers this.

## 6. Conformance

### Fixtures

`tests/fixtures/krusty/` holds 327 runs recorded from upstream's
`api/krusty/*_test.go`, which is kustomize's end-to-end suite (format in its
README). They are recorded, not hand-transcribed:
- `scripts/import-upstream.sh <ref>` clones kustomize;
- it hooks a recorder into the upstream test harness (`scripts/upstream/`);
- then it runs `go test ./krusty/`.

Every run's in-memory filesystem, options and output (or error) become a case.
Expected output is the test's own golden string, and so exactly what that
kustomize version produces. The same script also exports the default field-spec
tables to `src/builtin_config/`.

Refresh when kustomize releases: `just import <tag>`, then `just conformance`,
then review the STATUS.md diff. New failures mean upstream changed behavior.

`tests/fixtures/flux/` holds Flux's raw test data (section 8).

### Runner

`tests/conformance/main.rs` is a custom harness. It is its own test target
with `harness = false` and `test = false`.

- Each case gets one outcome:
  - **pass**;
  - **fail**: built, but wrong (a bug);
  - **unimplemented**: hit `Error::Unsupported`, the backlog;
  - **deferred**: plugins, on-disk tests, or an entry in `conformance/deferred.yaml`.
- Output is compared as data: each document parsed to `serde_json::Value`, in
  order. Document order is significant.
- An error case passes on any non-`Unsupported` error.
- A panic is caught and counted as a failure.

### Not a merge gate

Most cases fail until their features land, so conformance never blocks a merge:

- `cargo test` / `just ci` don't run it (`test = false`).
- `just conformance` always exits 0 and rewrites `conformance/STATUS.md`. That
  file is checked in, so progress shows in review.
- CI runs it in a separate job that isn't a required check, and posts
  STATUS.md to the job summary.

**The ratchet.** `conformance/baseline.txt` lists every case that passes. A
change may add to it, never lose one:
- `just regressions` (`--check-regressions`) exits 2 if a baseline case no
  longer passes;
- the CI conformance job goes red on a regression, which is visible but not
  merge-blocking;
- `krs check` rejects a delegated change that regresses;
- `just baseline` (`--write-baseline`) and `krs accept` move it up after review.

`--json <path>` writes per-case results for tooling. `--strict` fails unless
every in-scope case passes, for the far future.

**No special-casing.** `tests/fixture_leaks.rs` runs in `cargo test`, so it *is*
a gate. It collects the instance data the fixtures use (names, namespaces,
label and annotation values, images, prefixes) and fails if engine code
contains one of those strings, a fixture path, or an upstream test name.
Genuine Kubernetes vocabulary is allowlisted in `conformance/leak-allowlist.txt`,
each entry with a reason.

Unit tests (`just test`) are the gate: every implemented behavior gets focused
tests in its module, so regressions show there even while conformance is
incomplete.

## 7. Output fidelity

The output is equal to kustomize's **as data**, not byte for byte. Values pass
through `serde_json::Value`, which loses:

- scalar style: quoting, `0644` vs `420`, block vs flow;
- comments;
- YAML tags;
- integers beyond i64/u64.

Map key order follows insertion order. That's the input order unless a
transformer changes it, which matches kustomize in most cases.

That's fine for the main consumer: flux9s sends the result to the API server for
a dry-run, and diffs what comes back. If byte fidelity is ever needed, the
upgrade path is a node-level YAML model (like kyaml's `RNode`) behind the same
API. Don't start there.

YAML parsing rules (`src/yaml.rs`):
- YAML 1.2 booleans;
- kubectl-style octal;
- duplicate keys last-wins (as go-yaml does; upstream tests rely on it).

## 8. Flux layer (feature `flux`, later)

Flux steps that `flux build kustomization` / `flux diff kustomization` run
around kustomize. Source: `fluxcd/pkg/kustomize`.

- **F1: generator.** Given a Flux `Kustomization` spec (as `serde_json::Value`,
  so there's no k8s-openapi dependency):
  1. Find the kustomization file. If there isn't one, generate it:
     - scan for `.yaml`/`.yml` manifests that parse as Kubernetes objects;
     - respect `.sourceignore` and `spec.ignore`;
     - add subdirectories that hold a kustomization file as resources, without
       descending into them.
  2. Overlay these spec fields onto it:
     - `targetNamespace` → `namespace`;
     - `namePrefix` / `nameSuffix`;
     - `patches`;
     - `components` (local relative paths only; `ignoreMissingComponents`);
     - the deprecated patch fields;
     - `images`, merged field by field into existing entries;
     - `buildMetadata`.

  The result goes into a `MemoryFs` overlay rather than being written to disk.
- **F2: post-build substitution.** Port `fluxcd/pkg/envsubst`:
  - `${var}`, `${var:=default}` and the other bash-style operators;
  - strict mode;
  - the `kustomize.toolkit.fluxcd.io/substitute: disabled` annotation.

  The variables come from the caller (inline `substitute` plus resolved
  `substituteFrom`). Fetching ConfigMaps and Secrets from a cluster is the
  caller's job.
- **Out of scope:** SOPS decryption. Masking secret values in a diff is the
  consumer's job (flux9s).

Fixtures: `tests/fixtures/flux/testdata` is copied raw. Turning
`kustomize_generator_test.go` and `kustomize_varsub_test.go` into case files is
the first task of F1/F2.

## 9. Open questions

- Publish name: `kustomize-rs` is free on crates.io today.
- Remote resources: a resolver trait (the caller fetches), or built-in git via
  `gix`?
- Whether flux9s depends on this crate from crates.io or by git while it is
  pre-1.0.
- Whether the ratchet should become a required check once tier 1 is mostly green.
