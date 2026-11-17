//! Runs the recorded upstream kustomize cases (tests/fixtures/krusty) against
//! this crate and reports how many pass.
//!
//! This is a progress report, not a gate: it exits 0 however many cases fail,
//! unless `--strict` is given. Without a filter it rewrites
//! `conformance/STATUS.md`.
//!
//! ```text
//! cargo test --test conformance                      # full run, update STATUS.md
//! cargo test --test conformance -- namereference -v  # matching cases, with diffs
//! ```

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::{Path, PathBuf};

use kustomize::fs::MemoryFs;
use kustomize::{BuildOptions, Error, LoadRestrictions, Reorder, build, yaml};
use serde::Deserialize;
use serde_json::Value;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Case {
    run: Run,
    expect: String,
    filesystem: String,
    #[serde(default)]
    tweaked: bool,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Run {
    path: String,
    load_restrictions: String,
    reorder: String,
    add_managed_by_label: bool,
    plugins: String,
}

#[derive(Deserialize)]
struct Deferred {
    deferred: Vec<DeferredEntry>,
}

#[derive(Deserialize)]
struct DeferredEntry {
    prefix: String,
    reason: String,
}

enum Outcome {
    Pass,
    /// Built, but wrong: a real bug.
    Fail(String),
    /// Needs a feature that isn't implemented yet.
    Unsupported(String),
    /// Set aside on purpose (see conformance/deferred.yaml).
    Deferred(String),
}

struct Args {
    filter: Option<String>,
    verbose: bool,
    strict: bool,
}

fn parse_args() -> Args {
    let mut args = Args {
        filter: None,
        verbose: false,
        strict: false,
    };
    for arg in std::env::args().skip(1) {
        match arg.as_str() {
            "-v" | "--verbose" => args.verbose = true,
            "--strict" => args.strict = true,
            a if a.starts_with('-') => {} // ignore flags cargo may pass through
            a => args.filter = Some(a.to_string()),
        }
    }
    args
}

fn find_cases(dir: &Path, out: &mut Vec<PathBuf>) {
    let mut entries: Vec<_> = std::fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().path())
        .collect();
    entries.sort();
    for path in entries {
        if path.is_dir() {
            if path.join("case.yaml").is_file() {
                out.push(path);
            } else {
                find_cases(&path, out);
            }
        }
    }
}

fn run_case(dir: &Path, id: &str, deferred: &[DeferredEntry], verbose: bool) -> Outcome {
    let case: Case =
        yaml::from_str(&std::fs::read_to_string(dir.join("case.yaml")).unwrap()).unwrap();
    if let Some(d) = deferred.iter().find(|d| id.starts_with(&d.prefix)) {
        return Outcome::Deferred(d.reason.clone());
    }
    if case.run.plugins == "enabled" {
        return Outcome::Deferred("plugins".to_string());
    }
    if case.filesystem != "inMemory" {
        return Outcome::Deferred("on-disk test, input not recorded".to_string());
    }
    if case.tweaked {
        return Outcome::Deferred("upstream post-processed the output".to_string());
    }

    let options = BuildOptions {
        load_restrictions: match case.run.load_restrictions.as_str() {
            "none" => LoadRestrictions::None,
            _ => LoadRestrictions::RootOnly,
        },
        reorder: match case.run.reorder.as_str() {
            "none" => Reorder::None,
            _ => Reorder::Legacy,
        },
        add_managed_by_label: case.run.add_managed_by_label,
    };
    let input = dir.join("input");
    let fs = if input.is_dir() {
        MemoryFs::from_dir(&input).unwrap()
    } else {
        MemoryFs::default()
    };

    let result = match catch_unwind(AssertUnwindSafe(|| build(&fs, &case.run.path, &options))) {
        Ok(r) => r,
        Err(_) => return Outcome::Fail("panicked".to_string()),
    };

    match (case.expect.as_str(), result) {
        (_, Err(Error::Unsupported(feature))) => Outcome::Unsupported(feature),
        ("error", Err(_)) => Outcome::Pass,
        ("error", Ok(_)) => {
            let want = std::fs::read_to_string(dir.join("expected-error.txt")).unwrap_or_default();
            Outcome::Fail(format!(
                "expected an error: {}",
                want.lines().next().unwrap_or("")
            ))
        }
        (_, Err(e)) => Outcome::Fail(format!("error: {e}")),
        (_, Ok(m)) => {
            let text = std::fs::read_to_string(dir.join("expected.yaml")).unwrap_or_default();
            let want = match yaml::documents(&text) {
                Ok(w) => w,
                Err(e) => return Outcome::Fail(format!("cannot parse expected.yaml: {e}")),
            };
            let got = m.to_values();
            if got == want {
                Outcome::Pass
            } else {
                let mut detail = format!(
                    "output differs ({} documents, expected {})",
                    got.len(),
                    want.len()
                );
                if verbose {
                    detail.push('\n');
                    detail.push_str(&diff(&want, &got));
                }
                Outcome::Fail(detail)
            }
        }
    }
}

fn render(docs: &[Value]) -> String {
    docs.iter()
        .map(|d| yaml::to_string(d).unwrap_or_else(|e| format!("<unrenderable: {e}>\n")))
        .collect::<Vec<_>>()
        .join("---\n")
}

fn diff(want: &[Value], got: &[Value]) -> String {
    similar::TextDiff::from_lines(&render(want), &render(got))
        .unified_diff()
        .header("expected", "actual")
        .to_string()
}

#[derive(Default)]
struct Tally {
    pass: usize,
    fail: usize,
    unsupported: usize,
    deferred: usize,
}

fn main() {
    let args = parse_args();
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let fixtures = root.join("tests/fixtures/krusty");
    let deferred: Deferred =
        yaml::from_str(&std::fs::read_to_string(root.join("conformance/deferred.yaml")).unwrap())
            .unwrap();
    let commit = std::fs::read_to_string(fixtures.join("UPSTREAM_COMMIT")).unwrap_or_default();

    let mut dirs = Vec::new();
    find_cases(&fixtures, &mut dirs);

    // Keep panics in the engine from flooding the report; they're counted as failures.
    if !args.verbose {
        std::panic::set_hook(Box::new(|_| {}));
    }

    let mut total = Tally::default();
    let mut groups: BTreeMap<String, Tally> = BTreeMap::new();
    let mut blocked: BTreeMap<String, usize> = BTreeMap::new();
    let mut failures = Vec::new();
    let mut deferred_reasons: BTreeMap<String, usize> = BTreeMap::new();

    for dir in &dirs {
        let id = dir
            .strip_prefix(&fixtures)
            .unwrap()
            .to_string_lossy()
            .replace('\\', "/");
        if args
            .filter
            .as_ref()
            .is_some_and(|f| !id.contains(f.as_str()))
        {
            continue;
        }
        let group = id.split('/').next().unwrap_or_default().to_string();
        let tally = groups.entry(group).or_default();
        match run_case(dir, &id, &deferred.deferred, args.verbose) {
            Outcome::Pass => {
                tally.pass += 1;
                total.pass += 1;
                if args.verbose {
                    println!("PASS {id}");
                }
            }
            Outcome::Fail(why) => {
                tally.fail += 1;
                total.fail += 1;
                if args.filter.is_some() || args.verbose {
                    println!("FAIL {id}: {why}");
                }
                failures.push((id, why.lines().next().unwrap_or_default().to_string()));
            }
            Outcome::Unsupported(feature) => {
                tally.unsupported += 1;
                total.unsupported += 1;
                if args.filter.is_some() || args.verbose {
                    println!("TODO {id}: {feature}");
                }
                *blocked.entry(feature).or_default() += 1;
            }
            Outcome::Deferred(reason) => {
                tally.deferred += 1;
                total.deferred += 1;
                *deferred_reasons.entry(reason).or_default() += 1;
            }
        }
    }

    let in_scope = total.pass + total.fail + total.unsupported;
    let pct = if in_scope == 0 {
        0.0
    } else {
        100.0 * total.pass as f64 / in_scope as f64
    };
    let headline = format!(
        "{} / {in_scope} in-scope cases pass ({pct:.1}%). {} wait on unimplemented features, {} fail, {} deferred.",
        total.pass, total.unsupported, total.fail, total.deferred
    );
    println!("\n{headline}");

    if args.filter.is_none() {
        let mut out = String::new();
        let _ = writeln!(out, "# Conformance status\n");
        let _ = writeln!(
            out,
            "Generated by `just conformance`; do not edit. Cases recorded from kustomize `{}`.\n",
            commit.trim().get(..12).unwrap_or(commit.trim())
        );
        let _ = writeln!(out, "**{headline}**\n");
        let _ = writeln!(out, "## Unimplemented features, by cases waiting on them\n");
        let _ = writeln!(
            out,
            "Only the first unsupported feature a case hits is counted.\n"
        );
        let _ = writeln!(out, "| Feature | Cases |\n|---|---|");
        let mut by_count: Vec<_> = blocked.iter().collect();
        by_count.sort_by(|a, b| b.1.cmp(a.1).then(a.0.cmp(b.0)));
        for (feature, n) in by_count {
            let _ = writeln!(out, "| {feature} | {n} |");
        }
        let _ = writeln!(out, "\n## Failing cases\n");
        if failures.is_empty() {
            let _ = writeln!(out, "None.");
        }
        for (id, why) in &failures {
            let _ = writeln!(out, "- `{id}`: {why}");
        }
        let _ = writeln!(out, "\n## By upstream test file\n");
        let _ = writeln!(
            out,
            "| File | Pass | Unimplemented | Fail | Deferred |\n|---|---|---|---|---|"
        );
        for (group, t) in &groups {
            let _ = writeln!(
                out,
                "| {group} | {} | {} | {} | {} |",
                t.pass, t.unsupported, t.fail, t.deferred
            );
        }
        let _ = writeln!(out, "\n## Deferred\n\n| Reason | Cases |\n|---|---|");
        for (reason, n) in &deferred_reasons {
            let _ = writeln!(out, "| {reason} | {n} |");
        }
        std::fs::write(root.join("conformance/STATUS.md"), out).unwrap();
        println!("wrote conformance/STATUS.md");
    }

    if args.strict && total.pass != in_scope {
        std::process::exit(1);
    }
}
