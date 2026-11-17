// Copyright 2026 Dan Guns
// SPDX-License-Identifier: Apache-2.0
//
// Exports schema-derived tables from kustomize's kyaml/openapi package
// (Copyright The Kubernetes Authors, Apache-2.0) as JSON for kustomize-rs.
// scripts/import-upstream.sh copies it into a kustomize checkout.

package openapi

import (
	"encoding/json"
	"os"
	"path/filepath"
	"sort"
	"strings"
	"testing"

	"k8s.io/kube-openapi/pkg/validation/spec"
)

func TestDumpForKustomizeRs(t *testing.T) {
	dir := os.Getenv("KRS_DUMP_DIR")
	if dir == "" {
		t.Skip()
	}
	type entry struct {
		APIVersion string `json:"apiVersion"`
		Kind       string `json:"kind"`
		Namespaced bool   `json:"namespaced"`
	}
	var scope []entry
	for tm, namespaced := range precomputedIsNamespaceScoped {
		scope = append(scope, entry{tm.APIVersion, tm.Kind, namespaced})
	}
	sort.Slice(scope, func(i, j int) bool {
		if scope[i].APIVersion != scope[j].APIVersion {
			return scope[i].APIVersion < scope[j].APIVersion
		}
		return scope[i].Kind < scope[j].Kind
	})
	data, err := json.MarshalIndent(scope, "", "  ")
	if err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(filepath.Join(dir, "namespace-scope.json"), append(data, '\n'), 0o644); err != nil {
		t.Fatal(err)
	}
}

// The strategic-merge schema: every definition's properties, as compact nodes.
//
//	node := ""                    a known field with nothing more to say
//	      | "Name"                $ref to definition Name
//	      | {"t": "array", "i": node, "a": node, "p": {field: node},
//	         "s": strategy, "k": [merge keys]}
func TestDumpSmpSchemaForKustomizeRs(t *testing.T) {
	dir := os.Getenv("KRS_DUMP_DIR")
	if dir == "" {
		t.Skip()
	}
	root := Schema()
	defs := map[string]interface{}{}
	kinds := map[string]string{}
	for name, def := range root.Definitions {
		d := def
		defs[name] = encodeSchema(&d, true)
		if gvks, ok := def.Extensions["x-kubernetes-group-version-kind"].([]interface{}); ok {
			for _, g := range gvks {
				m, ok := g.(map[string]interface{})
				if !ok {
					continue
				}
				group, _ := m["group"].(string)
				version, _ := m["version"].(string)
				kind, _ := m["kind"].(string)
				apiVersion := version
				if group != "" {
					apiVersion = group + "/" + version
				}
				kinds[apiVersion+"|"+kind] = name
			}
		}
	}
	out := map[string]interface{}{
		"schemaVersion": GetSchemaVersion(),
		"kinds":         kinds,
		"definitions":   defs,
	}
	data, err := json.Marshal(out)
	if err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(filepath.Join(dir, "smp-schema.json"), append(data, '\n'), 0o644); err != nil {
		t.Fatal(err)
	}
}

func refName(s *spec.Schema) string {
	return strings.TrimPrefix(s.Ref.String(), "#/definitions/")
}

func encodeSchema(s *spec.Schema, top bool) interface{} {
	if s == nil {
		return ""
	}
	if !top && s.Ref.String() != "" {
		return refName(s)
	}
	n := map[string]interface{}{}
	if len(s.Type) == 1 && s.Type[0] == "array" {
		n["t"] = "array"
		if s.Items != nil && s.Items.Schema != nil {
			n["i"] = encodeSchema(s.Items.Schema, false)
		}
	}
	if s.AdditionalProperties != nil && s.AdditionalProperties.Schema != nil {
		n["a"] = encodeSchema(s.AdditionalProperties.Schema, false)
	}
	if len(s.Properties) > 0 {
		props := map[string]interface{}{}
		keys := make([]string, 0, len(s.Properties))
		for k := range s.Properties {
			keys = append(keys, k)
		}
		sort.Strings(keys)
		for _, k := range keys {
			p := s.Properties[k]
			props[k] = encodeSchema(&p, false)
		}
		n["p"] = props
	}
	rs := ResourceSchema{Schema: s}
	if strategy, keys := rs.PatchStrategyAndKeyList(); strategy != "" {
		n["s"] = strategy
		if len(keys) > 0 {
			n["k"] = keys
		}
	}
	if len(n) == 0 && !top {
		return ""
	}
	return n
}

