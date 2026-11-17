# Roadmap: hardest parts first

SPEC.md says **what** to build. This file says **in what order and by whom**.
The order is deliberately hardest-first: the lead (a strong model or a human)
builds the foundations and the parts that need design judgement. What's left is
mostly translating one upstream file against a clear verifier, which small
models can do. Each item is a task file in `tasks/`. `just krs next` shows
what's ready.

```
                 ┌──────────────────────────────────────────────┐
 Phase A (lead)  │ L-framework → L-fieldspec → L-smp-schema → L-smp-merge
 foundations     │                     │                         │
                 │                     ├──────→ L-nameref ←──────┤ (needs a renamer: W-prefix-suffix)
                 │                     └──────→ L-components, L-replacements
                 └──────────────────────────────────────────────┘
 Phase B (workers, unlocked as Phase A lands)
   after L-framework: W-metadata-strings, W-local-config, W-sort-order, W-selector
   after L-fieldspec: W-namespace, W-prefix-suffix, W-labels, W-annotations, W-images, W-replicas
   after W-selector:  W-json6902
   after L-smp-merge: W-patches-smp
   after L-framework: W-generators → W-hash-suffix (needs L-nameref to pass most cases)
 Phase C (open-ended): openapi custom schemas, vars, buildMetadata, managed-by, Flux layer F1/F2
```

## Phase A: foundations (lead)

These need design decisions. A mistake here gets copied into every worker task
that builds on it, so each one ships with an interface the workers code against,
plus thorough unit tests.

| Order | Task | Why it's hard | Unlocks |
|---|---|---|---|
| 10 | **L-framework** | Sets the shape of everything: the `Transformer` trait, mutable `ResMap` access, build state as `Resource` fields (original id, previous ids, generator behavior), typed kustomization fields, the transformer config (field-spec tables), and the stage wiring in `build.rs` in upstream order | all |
| 20 | **L-fieldspec** | `filters/fieldspec` + `fsslice` + `filtersutil`: path walking through maps and sequences, `create`, GVK matching, setters. Every builtin transformer is "apply a setter at these field specs" | all field-spec transformers |
| 30 | **L-smp-schema** | Strategic merge needs per-field merge keys and strategies. Generate a compact table from kustomize's embedded schema (v1.21.2), with a Go exporter in `scripts/upstream/`, and add a Rust lookup API | L-smp-merge |
| 40 | **L-smp-merge** | Port kyaml `merge2` + `walk`: maps, associative lists (merge keys), non-associative lists, scalars, null deletion, `$patch` directives. The most subtle code in kustomize. (kyaml implements no other SMP directive: `$retainKeys`, `$setElementOrder` and `$deleteFromPrimitiveList` pass through as plain fields, so there is no separate directives task.) | patches, components, generators merge |
| 50 | **L-nameref** | Renames propagate to referrers (`filters/nameref`): candidate selection by namespace, prefix/suffix lineage, role refs (including the blank-subject-namespace error), ambiguity errors | hash suffix, prefix/suffix cases, most overlays |
| 60 | **L-components** | `kind: Component` accumulates onto the parent's resources; generator `behavior: merge/replace` across bases; diamond imports | component, complexcomposition, generatormergeandreplace |
| 80 | **L-replacements** | `filters/replacement`: source/target selectors, field paths with list matching, delimiters, create | replacementtransformer |

## Phase B: delegated (workers)

Each is one upstream file (or two) plus examples. Expect a small model to need
one to four rounds. If a task fails twice, the lead splits it or takes it.

| Task | Upstream | Fixture groups |
|---|---|---|
| W-metadata-strings | `resource.RemoveBuildAnnotations` / `RNode.setMapInMetadata` | basic_io |
| W-local-config | `KustTarget.IgnoreLocal` | localconfig |
| W-selector | `types/selector.go`, `resid` matching | (used by patches, replacements) |
| W-namespace | `filters/namespace` | namespaces, namespacepropagation |
| W-prefix-suffix | `filters/prefix`, `filters/suffix` | basereusenameprefix, disablenamesuffix |
| W-labels | `filters/labels`, `LabelTransformer` | inlinelabels, numericcommonlabels |
| W-annotations | `filters/annotations` | |
| W-images | `filters/imagetag` | transformersimage |
| W-replicas | `filters/replicacount` | |
| W-json6902 | `filters/patchjson6902`, `PatchTransformer` JSON branch | gvknpatch |
| W-patches-smp | `PatchStrategicMergeTransformer`, `PatchTransformer` SMP branch | multiplepatch, inlinepatch |
| W-generators | `ConfigMapGenerator`, `SecretGenerator`, `api/kv`, `generators/utils.go` | configmaps, generatoroptions |
| W-hash-suffix | `api/hasher`, `HashTransformer` | configmaps |
| W-sort-order | `SortOrderTransformer` | sortordertransformer, legacy_order |

## Phase C: later

Tier-2 features and the Flux layer from SPEC.md, written up as tasks when
Phase B is mostly done:
- `openapi:` custom schemas;
- `vars`;
- `buildMetadata` origin annotations;
- the managed-by label;
- `configurations`/`crds`;
- F1 (Flux generator) and F2 (post-build substitution).

The order then comes from STATUS.md: the feature blocking the most cases goes
first.

## Done means

- **A task:** `krs check` says ACCEPT, the lead reviewed it, and a human committed it.
- **A phase:** its tasks are done.
- **The project:** when flux9s can build real Flux repositories with it (SPEC.md
  M6). The conformance score is how we measure the way there, and it never
  gates a merge.
