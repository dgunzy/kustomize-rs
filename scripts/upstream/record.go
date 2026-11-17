// Copyright 2026 Dan Guns
// SPDX-License-Identifier: Apache-2.0
//
// Recorder used to export kustomize's krusty tests as data fixtures for
// kustomize-rs. Not part of upstream kustomize: scripts/import-upstream.sh copies
// it into a kustomize checkout (package kusttest_test, Copyright The Kubernetes
// Authors, Apache-2.0) next to harness.patch. Active only when KRS_RECORD_DIR is set.

package kusttest_test

import (
	"fmt"
	"os"
	"path/filepath"
	"regexp"
	"runtime"
	"strings"
	"sync"
	"testing"

	"sigs.k8s.io/kustomize/api/krusty"
	"sigs.k8s.io/kustomize/api/resmap"
	"sigs.k8s.io/kustomize/api/types"
	"sigs.k8s.io/kustomize/kyaml/filesys"
)

var (
	recMu    sync.Mutex
	recCount = map[string]int{}
	recLast  = map[string]string{}
	unsafeRe = regexp.MustCompile(`[^A-Za-z0-9_.-]+`)
)

func recordDir() string { return os.Getenv("KRS_RECORD_DIR") }

// callerTestFile finds the *_test.go file that drove this run.
func callerTestFile() string {
	pcs := make([]uintptr, 32)
	n := runtime.Callers(2, pcs)
	frames := runtime.CallersFrames(pcs[:n])
	for {
		f, more := frames.Next()
		if strings.HasSuffix(f.File, "_test.go") {
			return filepath.Base(f.File)
		}
		if !more {
			return "unknown_test.go"
		}
	}
}

func sanitizeTestName(name string) string {
	parts := strings.Split(name, "/")
	for i, p := range parts {
		parts[i] = unsafeRe.ReplaceAllString(p, "_")
	}
	return filepath.Join(parts...)
}

func quote(s string) string { return fmt.Sprintf("%q", s) }

func recordRun(t *testing.T, fSys filesys.FileSystem, path string, o krusty.Options, m resmap.ResMap, runErr error) {
	root := recordDir()
	if root == "" {
		return
	}
	recMu.Lock()
	defer recMu.Unlock()

	file := callerTestFile()
	group := strings.TrimSuffix(file, "_test.go")
	key := t.Name()
	recCount[key]++
	dir := filepath.Join(root, group, sanitizeTestName(key), fmt.Sprintf("%02d", recCount[key]))
	recLast[key] = dir
	if err := os.MkdirAll(dir, 0o755); err != nil {
		t.Logf("record: %v", err)
		return
	}

	inMemory := fmt.Sprintf("%T", fSys) == "*filesys.fsNode"
	runPath := strings.TrimPrefix(filepath.Clean(path), "/")
	if runPath == "" {
		runPath = "."
	}
	if inMemory {
		_ = fSys.Walk("/", func(p string, info os.FileInfo, err error) error {
			if err != nil || info.IsDir() {
				return err
			}
			data, rerr := fSys.ReadFile(p)
			if rerr != nil {
				return rerr
			}
			dst := filepath.Join(dir, "input", strings.TrimPrefix(p, "/"))
			_ = os.MkdirAll(filepath.Dir(dst), 0o755)
			return os.WriteFile(dst, data, 0o644)
		})
	}

	plugins := "disabled"
	if o.PluginConfig != nil && (o.PluginConfig.PluginRestrictions != types.PluginRestrictionsBuiltinsOnly ||
		o.PluginConfig.FnpLoadingOptions.EnableExec || o.PluginConfig.HelmConfig.Enabled) {
		plugins = "enabled"
	}
	loadRestrictions := "rootOnly"
	if o.LoadRestrictions == types.LoadRestrictionsNone {
		loadRestrictions = "none"
	}
	reorder := string(o.Reorder)
	if reorder == "" {
		reorder = "none"
	}

	expect := "output"
	if runErr != nil {
		expect = "error"
		_ = os.WriteFile(filepath.Join(dir, "expected-error.txt"), []byte(runErr.Error()+"\n"), 0o644)
	} else if m != nil {
		if out, err := m.AsYaml(); err == nil {
			_ = os.WriteFile(filepath.Join(dir, "expected.yaml"), out, 0o644)
		}
	}

	var b strings.Builder
	fmt.Fprintf(&b, "# Generated from upstream kustomize. Do not edit by hand.\n")
	fmt.Fprintf(&b, "upstream:\n  file: %s\n  test: %s\n", quote("api/krusty/"+file), quote(key))
	fmt.Fprintf(&b, "run:\n  path: %s\n  loadRestrictions: %s\n  reorder: %s\n  addManagedByLabel: %t\n  plugins: %s\n",
		quote(runPath), loadRestrictions, reorder, o.AddManagedbyLabel, plugins)
	fmt.Fprintf(&b, "expect: %s\n", expect)
	fmt.Fprintf(&b, "filesystem: %s\n", map[bool]string{true: "inMemory", false: "onDisk"}[inMemory])
	_ = os.WriteFile(filepath.Join(dir, "case.yaml"), []byte(b.String()), 0o644)
}

// recordAssert marks the most recent run of this test as golden-asserted.
func recordAssert(t *testing.T, m resmap.ResMap, tweaked bool, expected string) {
	if recordDir() == "" {
		return
	}
	recMu.Lock()
	defer recMu.Unlock()
	dir, ok := recLast[t.Name()]
	if !ok {
		return
	}
	_ = os.WriteFile(filepath.Join(dir, "expected.yaml"), []byte(expected), 0o644)
	extra := "asserted: true\n"
	if tweaked {
		extra += "tweaked: true\n"
	}
	f, err := os.OpenFile(filepath.Join(dir, "case.yaml"), os.O_APPEND|os.O_WRONLY, 0o644)
	if err == nil {
		_, _ = f.WriteString(extra)
		_ = f.Close()
	}
	_ = m
}
