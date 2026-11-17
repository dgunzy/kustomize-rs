# Delegation

How a lead (a strong model like Claude or GPT, or a human) hands kustomize-rs
work to workers (small local models, subagents, contributors), checks it, and
accepts or rejects it, without anyone writing to git.

```
lead: plan ─▶ tasks/*.toml ─▶ krs brief ─▶ worker edits .cache/work/TASK
                                                │
lead: krs accept ◀── ACCEPT ◀── krs check ◀─────┘ (REJECT → feedback → worker retries)
  │
  └─▶ files copied into the repo, baseline ratcheted, task marked done
      ─▶ human reviews `git diff` and commits
```

## Roles

| Role | Does | Doesn't |
|---|---|---|
| **Lead** | Picks tasks, writes/splits task files, does the hard design tasks itself (`owner = "lead"`), reviews and accepts | Re-check what `krs check` already gates |
| **Worker** | Implements one task inside its workspace, until `krs check` says ACCEPT | Touch anything outside the task's scope, or git |
| **Human** | Reviews accepted changes, commits with sign-off | |

## The loop

```bash
just krs status                 # score, task states, what's ready
just krs next --owner worker    # tasks a small model can take
just krs workspace W-namespace  # isolated copy: .cache/work/W-namespace
just krs brief W-namespace --out .cache/brief.md   # the worker's whole context
# ... worker edits files in .cache/work/W-namespace ...
just krs check W-namespace      # ACCEPT or REJECT, with reasons
just krs accept W-namespace     # or: just krs reject W-namespace --reason "..."
```

### Automated worker against a local model

Any OpenAI-compatible endpoint works, for example llama.cpp server, vLLM,
Ollama, or a hosted API with `KRS_API_KEY` set:

```bash
just krs work W-namespace --endpoint http://inference.lan:8000/v1 --model qwen-coder --rounds 4
```

Each round sends the system prompt (`prompts/worker.md`), the brief, and only
the previous attempt with its feedback. Older rounds are dropped, so the
prompt stays inside a small model's context. The model answers with complete
files (`=== FILE: path === ... === END FILE ===`). Files outside the task's
scope are refused. The loop stops at the first ACCEPT. It never accepts on its
own: the lead reviews, then runs `krs accept`.

### Claude Code or Codex as the lead

- Give a subagent `krs brief TASK` and the workspace path, and tell it to stop
  when `krs check TASK` says ACCEPT.
- Or run `krs work` against a local endpoint and only read the verdict.

## Spending tokens wisely (for the lead)

- **Read summaries, not sources.** `krs status`, `krs next` and `krs check` each
  print a few lines. Open `.cache/krs/runs/TASK/latest.json` only when a verdict
  surprises you.
- **Let the brief carry the context.** `krs brief` already holds, in priority
  order and fitted to a token budget (`--budget`, default from the task):
  1. the files to edit, in full;
  2. the upstream Go, as named symbols rather than whole files;
  3. upstream's own unit-test cases for the filter (`units`), which the worker
     ports as a table-driven test;
  4. the read-only Rust it builds on, as an outline (docs and signatures);
  5. failing example cases.

  Don't read fixtures or upstream yourself to forward them.
- **Gates are mechanical; review is judgement.** Use `prompts/reviewer.md`'s four
  questions and nothing more.
- **Split before delegating.** Small models do well on tasks that need under
  about 300 lines of change, one upstream file, and an unambiguous verifier. If a
  worker fails twice, split the task or do it yourself. Don't keep re-rolling.
- **Hard parts first, by the lead.** ROADMAP.md orders the work so the lead
  builds the foundations and interfaces. The remaining tasks are then mostly
  translation, which workers handle well.

## Writing a task

`tasks/<id>.toml`. Ids start with `L-` (lead) or `W-` (worker).

```toml
id = "W-example"
title = "One line"
status = "todo"            # todo | in-progress | done | blocked
owner = "worker"           # lead | worker
difficulty = 2             # 1 (mechanical) .. 5 (design work)
order = 50                 # lower runs first among ready tasks
depends_on = ["L-fieldspec"]
summary = """
What to build and where it plugs in. Two to six sentences.
"""
notes = """
Optional: the lead's hints (pitfalls, which upstream branch matters).
"""

[context]
upstream = ["kustomize:api/filters/namespace/namespace.go#Filter,filter"]  # file or file#Symbols
read = ["src/fieldspec.rs"]          # Rust shown in full, read-only
cases = ["namespaces/"]               # fixture case-id prefixes or substrings
units = ["namespace_filter"]          # upstream unit tables (tests/fixtures/units/*.json)
max_examples = 2
budget = 12000                        # brief size in tokens

[scope]
write = ["src/transformers/namespace.rs", "src/transformers/mod.rs"]

[accept]
tests = ["transformers::namespace::"] # unit tests that must exist and pass
cases = []                            # case-id prefixes that must all pass
min_new_passes = 1
```

## What the gates check (`krs check`)

1. **Scope:** only `scope.write` paths changed, with no protected paths. The
   protected list is fixtures, conformance, tools, tasks, config, and the
   AGENTS/SPEC files.
2. **Attribution:** new `src/**.rs` files in a task with upstream context carry
   the "ported from" header.
3. **Standard checks:** `cargo fmt --check`, clippy with `-D warnings`, and
   `cargo test` (which includes the fixture-leak scan).
4. **The task's own criteria:** its `accept.tests` exist and pass.
5. **Conformance:**
   - no regressions against `conformance/baseline.txt`;
   - the cases in `accept.cases` pass;
   - at least `min_new_passes` cases newly pass.

`krs accept`:
1. copies the workspace into the repo;
2. rewrites the baseline (the ratchet only moves up);
3. marks the task done;
4. saves a patch to `.cache/krs/patches/`.

It never commits.
