#!/usr/bin/env python3
# Copyright 2026 Dan Guns
# SPDX-License-Identifier: Apache-2.0
"""Extract table-driven unit tests from kustomize filter tests into YAML fixtures.

Upstream filter tests (Copyright The Kubernetes Authors, Apache-2.0) are Go maps
of named cases with YAML in backtick strings. This turns the ones whose shape we
understand into data, so Rust unit tests can run exactly upstream's cases.

Usage: extract_filter_tables.py <kustomize checkout> <output dir>
"""

import json
import re
import sys
from pathlib import Path

# upstream test file -> output fixture, and how to read each case's filter.
TABLES = {
    "api/filters/fieldspec/fieldspec_test.go": "fieldspec_filter.json",
}
# kyaml merge2 test tables (`[]testCase{{description: ..., source, dest, expected, ...}}`).
MERGE2_TABLES = [
    "kyaml/yaml/merge2/element_test.go",
    "kyaml/yaml/merge2/list_test.go",
    "kyaml/yaml/merge2/map_test.go",
    "kyaml/yaml/merge2/scalar_test.go",
]
# Any `map[string]struct{...}{"name": {...}}` table: backtick fields become data,
# the rest of the case (filter settings) is kept as Go text under "go".
GENERIC_TABLES = {
    "api/filters/namespace/namespace_test.go": "namespace_filter.json",
    "api/filters/labels/labels_test.go": "labels_filter.json",
    "api/filters/annotations/annotations_test.go": "annotations_filter.json",
    "api/filters/imagetag/imagetag_test.go": "imagetag_filter.json",
    "api/filters/imagetag/legacy_test.go": "imagetag_legacy_filter.json",
    "api/filters/replicacount/replicacount_test.go": "replicacount_filter.json",
    "api/filters/patchstrategicmerge/patchstrategicmerge_test.go": "patchstrategicmerge_filter.json",
    "api/filters/nameref/nameref_test.go": "nameref_filter.json",
    "api/filters/replacement/replacement_test.go": "replacement_filter.json",
    "api/filters/refvar/refvar_test.go": "refvar_filter.json",
}
GENERIC_CASE = re.compile(r'^(?P<indent>\t+)"(?P<name>[^"\n]+)": \{\n(?P<body>.*?)^(?P=indent)\},?\n', re.S | re.M)
# The same, written as a slice of structs with a `name:` field.
GENERIC_SLICE_CASE = re.compile(r'^(?P<indent>\t+)\{\n(?P=indent)\tname:\s*"(?P<name>[^"\n]+)",\n(?P<body>.*?)^(?P=indent)\},?\n', re.S | re.M)
ANY_BACKTICK = re.compile(r'(?P<key>\w+):\s*`(?P<value>[^`]*)`', re.S)

MERGE2_CASE = re.compile(r"\{description: `(?P<d>[^`]*)`,(?P<body>.*?)(?=\n\t\{description: |\n\}\n)", re.S)
BACKTICK = re.compile(r"\b(?P<key>source|dest|expected):\s*`(?P<value>[^`]*)`", re.S)

CASE = re.compile(r'^\t\t"(?P<name>[^"]+)": \{\n(?P<body>.*?)^\t\t\},\n', re.S | re.M)
STRING_FIELD = re.compile(r'^\t\t\t(?P<key>\w+):\s*`(?P<value>.*?)`,', re.S | re.M)
SET_SCALAR = re.compile(r'SetValue:\s*filtersutil\.SetScalar\("(?P<v>[^"]*)"\)')
CREATE_KIND = re.compile(r'CreateKind:\s*yaml\.(?P<k>\w+)Node')
CREATE_TAG = re.compile(r'CreateTag:\s*yaml\.NodeTag(?P<t>\w+)')


def extract(go: str) -> list[dict]:
    body = go.split("testCases := map[string]struct", 1)[1]
    cases = []
    for m in CASE.finditer(body):
        b = m.group("body")
        case = {"name": m.group("name")}
        for f in STRING_FIELD.finditer(b):
            case[f.group("key")] = f.group("value").strip("\n") + "\n" if f.group("key") != "error" else f.group("value")
        setter = SET_SCALAR.search(b)
        if not setter:
            case["unsupported"] = "filter shape not understood"
        else:
            case["setScalar"] = setter.group("v")
        kind = CREATE_KIND.search(b)
        if kind:
            case["createKind"] = kind.group("k").lower()
        tag = CREATE_TAG.search(b)
        if tag:
            case["createTag"] = tag.group("t").lower()
        cases.append(case)
    return cases


def extract_generic(go: str) -> list[dict]:
    cases = []
    matches = list(GENERIC_CASE.finditer(go)) or list(GENERIC_SLICE_CASE.finditer(go))
    for m in matches:
        body = m.group("body")
        case = {"name": m.group("name")}
        for f in ANY_BACKTICK.finditer(body):
            case[f.group("key")] = f.group("value").strip("\n") + "\n"
        rest = ANY_BACKTICK.sub(lambda f: f"{f.group('key')}: <{f.group('key')}>", body)
        rest = "\n".join(line.strip() for line in rest.splitlines() if line.strip())
        if rest:
            case["go"] = rest
        cases.append(case)
    return cases


def extract_merge2(go: str, file: str) -> list[dict]:
    cases = []
    for m in MERGE2_CASE.finditer(go):
        body = m.group("body")
        case = {"file": file, "description": m.group("d")}
        for f in BACKTICK.finditer(body):
            case[f.group("key")] = f.group("value").strip("\n") + "\n"
        case["prepend"] = "MergeOptionsListPrepend" in body
        case["infer"] = bool(re.search(r"\binfer:\s*true", body))
        cases.append(case)
    return cases


def main():
    src, out = Path(sys.argv[1]), Path(sys.argv[2])
    out.mkdir(parents=True, exist_ok=True)
    merge2 = []
    for test_file in MERGE2_TABLES:
        merge2 += extract_merge2((src / test_file).read_text(), test_file)
    (out / "merge2.json").write_text(json.dumps({"upstream": "kyaml/yaml/merge2", "cases": merge2}, indent=2) + "\n")
    print(f"merge2.json: {len(merge2)} cases")
    for test_file, fixture in GENERIC_TABLES.items():
        path = src / test_file
        if not path.exists():
            print(f"{fixture}: missing upstream file {test_file}")
            continue
        cases = extract_generic(path.read_text())
        (out / fixture).write_text(json.dumps({"upstream": test_file, "cases": cases}, indent=2) + "\n")
        print(f"{fixture}: {len(cases)} cases")
    for test_file, fixture in TABLES.items():
        cases = extract((src / test_file).read_text())
        doc = {"upstream": test_file, "cases": cases}
        (out / fixture).write_text(json.dumps(doc, indent=2) + "\n")
        print(f"{fixture}: {len(cases)} cases")


if __name__ == "__main__":
    main()
