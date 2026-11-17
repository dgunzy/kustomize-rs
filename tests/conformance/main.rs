//! Runs the recorded upstream kustomize cases (tests/fixtures/krusty) against
//! this crate and reports how many pass.
//!
//! This is a progress report, not a merge gate: it exits 0 however many cases
//! fail. Without a filter it rewrites `conformance/STATUS.md`.
//!
//! ```text
//! cargo test --test conformance                        # full run, update STATUS.md
//! cargo test --test conformance -- namereference -v    # matching cases, with diffs
//! cargo test --test conformance -- --json out.json     # machine-readable results
//! cargo test --test conformance -- --check-regressions # exit 2 if a baseline case stopped passing
//! cargo test --test conformance -- --write-baseline    # record today's passing cases
//! ```
//!
//! The baseline (`conformance/baseline.txt`) is the ratchet: case ids that
//! passed when it was last written. A change may add passes, never lose one.
//! The delegation tooling (`tools/krs.py`) uses it to reject regressions.

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
    json: Option<PathBuf>,
    check_regressions: bool,
    write_baseline: bool,
    no_status: bool,
}

fn parse_args() -> Args {
    let mut args = Args {
        filter: None,
        verbose: false,
        strict: false,
        json: None,
        check_regressions: false,
        write_baseline: false,
        no_status: false,
    };
    let mut it = std::env::args().skip(1);
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "-v" | "--verbose" => args.verbose = true,
            "--strict" => args.strict = true,
            "--json" => args.json = it.next().map(PathBuf::from),
            "--check-regressions" => args.check_regressions = true,
            "--write-baseline" => args.write_baseline = true,
            "--no-status" => args.no_status = true,
            a if a.starts_with('-') => {} // ignore flags cargo may pass through
            a => args.filter = Some(a.to_string()),
        }
    }
    args
}

/// Case ids in the baseline file (one per line, `#` comments allowed).
fn read_baseline(path: &Path) -> Vec<String> {
    std::fs::read_to_string(path)
        .unwrap_or_default()
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .map(String::from)
        .collect()
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
    // Runtime value first: cargo sets it when it runs the test, so a binary reused
    // from another checkout still reads this tree's fixtures.
    let root_buf = std::env::var_os("CARGO_MANIFEST_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")));
    let root = root_buf.as_path();
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
    // (id, outcome, detail) for --json and the baseline.
    let mut results: Vec<(String, &str, String)> = Vec::new();

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
        let outcome = run_case(dir, &id, &deferred.deferred, args.verbose);
        let (kind, detail) = match &outcome {
            Outcome::Pass => ("pass", String::new()),
            Outcome::Fail(why) => ("fail", why.lines().next().unwrap_or_default().to_string()),
            Outcome::Unsupported(feature) => ("unimplemented", feature.clone()),
            Outcome::Deferred(reason) => ("deferred", reason.clone()),
        };
        results.push((id.clone(), kind, detail));
        match outcome {
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

    if let Some(path) = &args.json {
        let cases: Vec<_> = results
            .iter()
            .map(|(id, outcome, detail)| serde_json::json!({"id": id, "outcome": outcome, "detail": detail}))
            .collect();
        let report = serde_json::json!({
            "upstreamCommit": commit.trim(),
            "filter": args.filter,
            "summary": {
                "pass": total.pass,
                "fail": total.fail,
                "unimplemented": total.unsupported,
                "deferred": total.deferred,
                "inScope": in_scope,
            },
            "unimplementedFeatures": blocked,
            "cases": cases,
        });
        std::fs::write(path, serde_json::to_string_pretty(&report).unwrap()).unwrap();
        println!("wrote {}", path.display());
    }

    let baseline_path = root.join("conformance/baseline.txt");
    let mut regressed = false;
    if args.check_regressions {
        let outcome_of: BTreeMap<&str, &str> = results
            .iter()
            .map(|(id, outcome, _)| (id.as_str(), *outcome))
            .collect();
        let baseline = read_baseline(&baseline_path);
        let mut lost = Vec::new();
        for id in &baseline {
            if args
                .filter
                .as_ref()
                .is_some_and(|f| !id.contains(f.as_str()))
            {
                continue;
            }
            match outcome_of.get(id.as_str()) {
                Some(&"pass") => {}
                Some(other) => lost.push(format!("{id} ({other})")),
                None => lost.push(format!("{id} (missing)")),
            }
        }
        let gained = results
            .iter()
            .filter(|(id, outcome, _)| *outcome == "pass" && !baseline.contains(id))
            .count();
        if lost.is_empty() {
            println!("no regressions against the baseline; {gained} newly passing");
        } else {
            regressed = true;
            println!("REGRESSIONS: {} baseline cases no longer pass:", lost.len());
            for l in &lost {
                println!("  {l}");
            }
        }
    }

    if args.write_baseline {
        if args.filter.is_some() {
            println!("--write-baseline ignored: needs a full run (no filter)");
        } else {
            let mut out = String::from(
                "# Conformance ratchet: cases that pass. Written by `just baseline`; do not edit.\n\
                 # A change may add cases here, never lose one (`just regressions`).\n",
            );
            for (id, outcome, _) in &results {
                if *outcome == "pass" {
                    out.push_str(id);
                    out.push('\n');
                }
            }
            std::fs::write(&baseline_path, out).unwrap();
            println!("wrote conformance/baseline.txt ({} cases)", total.pass);
        }
    }

    if args.filter.is_none() && !args.no_status {
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

    if regressed {
        std::process::exit(2);
    }
    if args.strict && total.pass != in_scope {
        std::process::exit(1);
    }
}
