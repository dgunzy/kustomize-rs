# Reviewing a worker's change

`krs check` already enforces the mechanical gates: scope, fmt, clippy, tests, the
fixture-leak scan, attribution headers, no conformance regressions, and the task's
acceptance criteria. Don't spend tokens re-verifying those. Review what they can't:

1. **Faithful to upstream?** Skim the diff next to the upstream function named in
   the task. Watch for skipped branches, especially error paths and the empty, nil
   and list cases.
2. **Tests are real?** Unit tests should assert behavior from the upstream code,
   not just echo the implementation. A test that can't fail is a reject.
3. **Hidden special-casing?** Look for conditions on resource kinds or names that
   aren't in upstream's field-spec tables or code.
4. **Attribution honest?** A new file marked "not ported" shouldn't be a
   translation of an upstream file.
5. **Shape fits the crate?** Behavior goes through the field-spec engine and the
   transformer trait. Build state goes in `Resource` fields, never annotations. No
   new dependencies unless the task allows them.

Cheapest way to look: `diff -ruN -x target -x .git -x .cache . .cache/work/<task>`,
or the patch `krs accept` writes. Then `krs accept <task>`, or
`krs reject <task> --reason "..."` with a reason a worker can act on.
