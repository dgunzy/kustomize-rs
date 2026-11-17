// Copyright 2026 Dan Guns
// SPDX-License-Identifier: Apache-2.0
//
// Exports kustomize's builtin field-spec tables (package builtinpluginconsts,
// Copyright The Kubernetes Authors, Apache-2.0) as YAML for kustomize-rs.
// scripts/import-upstream.sh copies it into a kustomize checkout.

package builtinpluginconsts

import (
	"os"
	"path/filepath"
	"testing"
)

func TestDumpForKustomizeRs(t *testing.T) {
	dir := os.Getenv("KRS_DUMP_DIR")
	if dir == "" {
		t.Skip()
	}
	for name, body := range GetDefaultFieldSpecsAsMap() {
		if err := os.WriteFile(filepath.Join(dir, name+".yaml"), []byte(body), 0o644); err != nil {
			t.Fatal(err)
		}
	}
}
