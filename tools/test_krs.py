"""Tests for the delegation tool: run with `just tools-test`."""

import json
import sys
import tempfile
import unittest
from pathlib import Path

sys.dont_write_bytecode = True
sys.path.insert(0, str(Path(__file__).parent))
import krs  # noqa: E402


class GoSymbols(unittest.TestCase):
    SRC = """package x

// Filter does things.
type Filter struct {
\tA string
}

// Filter applies it.
func (f Filter) Filter(n int) int {
\treturn n
}

func other() {}
"""

    def test_returns_every_declaration_with_the_name_and_its_comments(self):
        out = krs.go_symbols(self.SRC, ["Filter"])
        self.assertIn("type Filter struct", out)
        self.assertIn("func (f Filter) Filter(n int) int {", out)
        self.assertIn("// Filter applies it.", out)
        self.assertNotIn("func other", out)

    def test_reports_missing_symbols(self):
        self.assertIn("not found", krs.go_symbols(self.SRC, ["Nope"]))


class RustOutline(unittest.TestCase):
    def test_elides_bodies_and_drops_tests(self):
        src = """/// Doc.
pub fn a(x: u8) -> u8 {
    if x > 1 {
        x
    } else {
        0
    }
}

pub struct S {
    pub f: u8,
}

impl S {
    /// Method doc.
    pub fn m(&self) -> u8 {
        self.f
    }
}

#[cfg(test)]
mod tests {
    fn t() {}
}
"""
        out = krs.rust_outline(src)
        self.assertIn("pub fn a(x: u8) -> u8 { ... }", out)
        self.assertIn("pub fn m(&self) -> u8 { ... }", out)
        self.assertIn("pub f: u8,", out)
        self.assertIn("/// Method doc.", out)
        self.assertNotIn("x > 1", out)
        self.assertNotIn("mod tests", out)


class Tasks(unittest.TestCase):
    def test_every_task_file_is_valid_and_dependencies_exist(self):
        tasks = krs.load_tasks()
        self.assertTrue(tasks)
        for t in tasks.values():
            for key in ("id", "title", "status", "owner", "summary", "scope"):
                self.assertIn(key, t, f"{t['id']}: missing {key}")
            self.assertIn(t["owner"], ("lead", "worker"), t["id"])
            self.assertIn(t["status"], ("todo", "in-progress", "done", "blocked"), t["id"])
            for dep in t.get("depends_on", []):
                self.assertIn(dep, tasks, f"{t['id']} depends on unknown task {dep}")
            for unit in t.get("context", {}).get("units", []):
                self.assertTrue((krs.ROOT / "tests/fixtures/units" / f"{unit}.json").exists(), f"{t['id']}: {unit}")

    def test_no_dependency_cycles(self):
        tasks = krs.load_tasks()

        def visit(tid, stack):
            self.assertNotIn(tid, stack, f"cycle: {' -> '.join(stack + [tid])}")
            for dep in tasks[tid].get("depends_on", []):
                visit(dep, stack + [tid])

        for tid in tasks:
            visit(tid, [])


class Scope(unittest.TestCase):
    def test_protected_paths_and_globs(self):
        task = {"scope": {"write": ["src/transformers/*.rs", "src/build.rs"]}}
        problems = krs.gate_scope(task, {
            "src/transformers/x.rs": "added",
            "src/build.rs": "modified",
            "src/lib.rs": "modified",
            "tests/fixtures/krusty/a/case.yaml": "modified",
        })
        self.assertEqual(len(problems), 2)
        self.assertTrue(any("src/lib.rs" in p for p in problems))


class ReplyParsing(unittest.TestCase):
    def test_writes_only_in_scope_files_and_strips_fences(self):
        task = {"scope": {"write": ["src/a.rs"]}}
        reply = "x\n=== FILE: src/a.rs ===\n```rust\nfn a() {}\n```\n=== END FILE ===\n=== FILE: Cargo.toml ===\nbad\n=== END FILE ===\n"
        with tempfile.TemporaryDirectory() as d:
            written, refused = krs.apply_reply(task, Path(d), reply)
            self.assertEqual(written, ["src/a.rs"])
            self.assertEqual(refused, ["Cargo.toml"])
            self.assertEqual((Path(d) / "src/a.rs").read_text(), "fn a() {}\n")


class Briefs(unittest.TestCase):
    def test_every_task_brief_builds_within_budget(self):
        for tid, t in krs.load_tasks().items():
            budget = t.get("context", {}).get("budget", 12000)
            text = krs.build_brief(t, budget)
            self.assertIn(f"# Task {tid}", text)
            # A truncated section may overshoot slightly; never by much.
            self.assertLess(krs.est_tokens(text), budget * 1.1, tid)


if __name__ == "__main__":
    unittest.main()
