//! Guards against "passing" upstream cases by hardcoding their data.
//!
//! Collects the instance-specific values the recorded fixtures use (resource
//! names, namespaces, label and annotation values, images, prefixes) and fails
//! if a string literal in the engine's non-test code equals one of them, or
//! names a fixture path or upstream test. A real implementation reads these
//! from its input; it never needs to spell them.
//!
//! Kubernetes vocabulary that legitimately appears in both (`default`,
//! `kube-system`, ...) goes in `conformance/leak-allowlist.txt` with a reason.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use serde_json::Value;

/// Keys whose string values are instance data in a fixture.
const VALUE_KEYS: [&str; 8] = [
    "name",
    "namespace",
    "generateName",
    "namePrefix",
    "nameSuffix",
    "image",
    "newName",
    "newTag",
];
/// Keys whose map values are all instance data.
const MAP_KEYS: [&str; 2] = ["labels", "annotations"];
/// Shorter values are too generic to mean anything.
const MIN_LEN: usize = 4;
/// A literal *containing* a value this long is also flagged.
const MIN_CONTAINED_LEN: usize = 12;

fn root() -> PathBuf {
    std::env::var_os("CARGO_MANIFEST_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")))
}

fn files(dir: &Path, ext: &[&str], out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            files(&path, ext, out);
        } else if path
            .extension()
            .and_then(|e| e.to_str())
            .is_some_and(|e| ext.contains(&e))
            || (ext.contains(&"") && path.file_name().is_some_and(|n| n == "Kustomization"))
        {
            out.push(path);
        }
    }
}

fn harvest(value: &Value, out: &mut BTreeSet<String>) {
    match value {
        Value::Object(map) => {
            for (key, v) in map {
                match v {
                    Value::String(s) if VALUE_KEYS.contains(&key.as_str()) => {
                        out.insert(s.clone());
                    }
                    Value::Object(m) if MAP_KEYS.contains(&key.as_str()) => {
                        out.extend(m.values().filter_map(|x| x.as_str().map(String::from)));
                    }
                    _ => harvest(v, out),
                }
            }
        }
        Value::Array(items) => items.iter().for_each(|v| harvest(v, out)),
        _ => {}
    }
}

/// Values the fixtures use as instance data, mapped to one fixture using each.
fn fixture_values() -> BTreeMap<String, String> {
    let mut paths = Vec::new();
    files(
        &root().join("tests/fixtures"),
        &["yaml", "yml", ""],
        &mut paths,
    );
    let mut out = BTreeMap::new();
    for path in paths {
        let Ok(text) = std::fs::read_to_string(&path) else {
            continue;
        };
        let Ok(docs) = kustomize::yaml::documents(&text) else {
            continue;
        };
        let mut found = BTreeSet::new();
        docs.iter().for_each(|d| harvest(d, &mut found));
        let rel = path
            .strip_prefix(root())
            .unwrap_or(&path)
            .display()
            .to_string();
        for v in found {
            out.entry(v).or_insert_with(|| rel.clone());
        }
    }
    out
}

/// Words that are Kubernetes vocabulary, not fixture data.
fn vocabulary() -> BTreeSet<String> {
    let mut words = BTreeSet::new();
    let mut tables = Vec::new();
    files(&root().join("src/builtin_config"), &["yaml"], &mut tables);
    for path in tables {
        let text = std::fs::read_to_string(&path).unwrap_or_default();
        words.extend(
            text.split(|c: char| !(c.is_alphanumeric() || "-_./".contains(c)))
                .filter(|w| !w.is_empty())
                .map(String::from),
        );
    }
    let allow =
        std::fs::read_to_string(root().join("conformance/leak-allowlist.txt")).unwrap_or_default();
    for line in allow.lines() {
        let word = line.split('#').next().unwrap_or("").trim();
        if !word.is_empty() {
            words.insert(word.to_string());
        }
    }
    words
}

/// String literals in Rust source, skipping comments (doc examples are tests)
/// and everything from the first `#[cfg(test)]` on.
fn literals(source: &str) -> Vec<(usize, String)> {
    let code = source.split("\n#[cfg(test)]").next().unwrap_or(source);
    let chars: Vec<char> = code.chars().collect();
    let mut out = Vec::new();
    let mut line = 1;
    let mut i = 0;
    let at = |i: usize| chars.get(i).copied().unwrap_or('\0');
    while i < chars.len() {
        let c = chars[i];
        match c {
            '\n' => line += 1,
            '/' if at(i + 1) == '/' => {
                while i < chars.len() && chars[i] != '\n' {
                    i += 1;
                }
                continue;
            }
            '/' if at(i + 1) == '*' => {
                let mut depth = 0;
                while i < chars.len() {
                    if chars[i] == '/' && at(i + 1) == '*' {
                        depth += 1;
                        i += 1;
                    } else if chars[i] == '*' && at(i + 1) == '/' {
                        depth -= 1;
                        i += 1;
                        if depth == 0 {
                            i += 1;
                            break;
                        }
                    } else if chars[i] == '\n' {
                        line += 1;
                    }
                    i += 1;
                }
                continue;
            }
            '\'' => {
                // Char literal ('x', '\n', '"') or a lifetime ('a).
                if at(i + 1) == '\\' {
                    i += 2;
                    while i < chars.len() && chars[i] != '\'' {
                        i += 1;
                    }
                } else if at(i + 2) == '\'' {
                    i += 2;
                }
            }
            'r' | 'b'
                if !at(i.wrapping_sub(1)).is_alphanumeric() && at(i.wrapping_sub(1)) != '_' =>
            {
                // Raw strings: r"..", r#".."#, br"..".
                let mut j = i + 1;
                if c == 'b' && at(j) == 'r' {
                    j += 1;
                }
                let mut hashes = 0;
                while at(j) == '#' {
                    hashes += 1;
                    j += 1;
                }
                if (c == 'r' || at(i + 1) == 'r') && at(j) == '"' {
                    let start_line = line;
                    let close: String = std::iter::once('"')
                        .chain(std::iter::repeat_n('#', hashes))
                        .collect();
                    let rest: String = chars[j + 1..].iter().collect();
                    let end = rest.find(&close).unwrap_or(rest.len());
                    let lit: String = rest[..end].to_string();
                    line += lit.matches('\n').count();
                    out.push((start_line, lit.clone()));
                    i = j + 1 + rest[..end].chars().count() + close.chars().count();
                    continue;
                }
            }
            '"' => {
                let start_line = line;
                let mut lit = String::new();
                i += 1;
                while i < chars.len() && chars[i] != '"' {
                    if chars[i] == '\\' {
                        match at(i + 1) {
                            'n' => lit.push('\n'),
                            't' => lit.push('\t'),
                            '\n' => {
                                line += 1;
                                i += 2;
                                while at(i).is_whitespace() {
                                    if at(i) == '\n' {
                                        line += 1;
                                    }
                                    i += 1;
                                }
                                continue;
                            }
                            other => lit.push(other),
                        }
                        i += 2;
                        continue;
                    }
                    if chars[i] == '\n' {
                        line += 1;
                    }
                    lit.push(chars[i]);
                    i += 1;
                }
                out.push((start_line, lit));
            }
            _ => {}
        }
        i += 1;
    }
    out
}

#[test]
fn literal_extraction_skips_comments_chars_and_tests() {
    let src = "// \"no\"\nlet a = \"yes\"; let q = '\"'; fn f<'a>() {}\nlet r = r#\"raw \"x\"\"#;\n/* \"no\" */\n#[cfg(test)]\nmod t { \"no\" }\n";
    let found: Vec<String> = literals(src).into_iter().map(|(_, l)| l).collect();
    assert_eq!(found, ["yes", "raw \"x\""]);
}

#[test]
fn engine_code_does_not_hardcode_fixture_data() {
    let values = fixture_values();
    assert!(
        values.len() > 100,
        "harvested too few fixture values: {}",
        values.len()
    );
    let vocab = vocabulary();
    let test_names: BTreeSet<String> = {
        let mut cases = Vec::new();
        files(&root().join("tests/fixtures/krusty"), &["yaml"], &mut cases);
        cases
            .iter()
            .filter(|p| p.file_name().is_some_and(|n| n == "case.yaml"))
            .filter_map(|p| {
                let rel = p.strip_prefix(root().join("tests/fixtures/krusty")).ok()?;
                rel.components()
                    .nth(1)
                    .map(|c| c.as_os_str().to_string_lossy().into_owned())
            })
            .collect()
    };

    let mut sources = Vec::new();
    files(&root().join("src"), &["rs"], &mut sources);
    let mut leaks = Vec::new();
    for path in sources {
        let text = std::fs::read_to_string(&path).unwrap();
        let rel = path.strip_prefix(root()).unwrap().display().to_string();
        for (line, lit) in literals(&text) {
            let why = if lit.contains("fixtures/") {
                Some("names the fixture directory".to_string())
            } else if let Some(t) = test_names.iter().find(|t| lit.contains(t.as_str())) {
                Some(format!("names upstream test {t}"))
            } else if lit.len() >= MIN_LEN && !vocab.contains(&lit) {
                values
                    .get(&lit)
                    .map(|f| format!("fixture value, e.g. in {f}"))
                    .or_else(|| {
                        values
                            .iter()
                            .find(|(v, _)| {
                                v.len() >= MIN_CONTAINED_LEN
                                    && !vocab.contains(*v)
                                    && lit.contains(v.as_str())
                            })
                            .map(|(v, f)| format!("contains fixture value {v:?}, e.g. in {f}"))
                    })
            } else {
                None
            };
            if let Some(why) = why {
                leaks.push(format!("{rel}:{line}: {lit:?}: {why}"));
            }
        }
    }
    assert!(
        leaks.is_empty(),
        "engine code hardcodes fixture data (read it from the input instead; if it is \
         genuine Kubernetes vocabulary, add it to conformance/leak-allowlist.txt with a reason):\n{}",
        leaks.join("\n")
    );
}
