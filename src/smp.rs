// Portions of this file are ported from kustomize kyaml/yaml/merge2/{merge2,smpdirective}.go,
// kyaml/yaml/walk/{walk,map,associative_sequence,nonassociative_sequence,scalar}.go,
// kyaml/yaml/schema/schema.go and the element helpers in kyaml/yaml/{fns,rnode}.go,
// Copyright The Kubernetes Authors, licensed under Apache-2.0.
// Modified: translated from Go to Rust and adapted to this crate.

//! Strategic merge patch, as kustomize runs it (kyaml's `merge2`).
//!
//! - Maps merge field by field. A `null` in the patch deletes the field.
//! - Lists the schema marks `merge` combine element by element, matched on
//!   the merge keys (or by value, for lists of scalars). New elements go first
//!   (kustomize merges with "prepend"). Other lists, and every list of a kind
//!   the schema doesn't know (such as a custom resource), are replaced.
//! - `$patch: delete | replace | merge` on a map, or as a list element.
//!   kyaml implements no other directive: `$retainKeys`, `$setElementOrder/…`
//!   and `$deleteFromPrimitiveList/…` stay in the output as ordinary fields.
//!
//! The port keeps kyaml's quirks, since output must match: list elements are
//! merged into the destination in place, so the merged list takes the patch's
//! element order, and a `$patch: replace` on a single list element has no
//! effect.

use std::collections::BTreeSet;

use serde_json::{Map, Value};

use crate::openapi::Schema;
use crate::setters::{Tag, replace_scalar, set_map_entry};
use crate::{Error, Result};

const DIRECTIVE: &str = "$patch";
/// Keys kyaml tries when inferring a list's merge key (`AssociativeSequenceKeys`).
const INFERRED_KEYS: [&str; 1] = ["name"];

/// How to merge (kyaml's `MergeOptions` plus the walker's inference flag).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MergeOptions {
    /// Put new list elements before existing ones (kustomize: true).
    pub prepend: bool,
    /// Treat schema-less lists whose elements all have a `name` as merging on
    /// it (kustomize: false).
    pub infer_lists: bool,
}

impl MergeOptions {
    /// The options kustomize's patch transformers use.
    pub fn kustomize() -> Self {
        Self {
            prepend: true,
            infer_lists: false,
        }
    }
}

/// Merge `patch` into the resource `dest`. Returns `None` when the patch
/// deletes the whole resource (`$patch: delete` at the top).
pub fn merge(dest: &Value, patch: &Value, options: MergeOptions) -> Result<Option<Value>> {
    let merged = Walker { options }.walk(Some(dest.clone()), Some(patch.clone()), None)?;
    Ok(merged.into_field_value())
}

/// The outcome of merging one node.
struct Merged {
    /// `None` clears the field.
    value: Option<Value>,
    /// A null that stays in the output (kyaml's persistent null); other nulls
    /// clear the field they're set on.
    keep_null: bool,
    /// The result is the destination node, merged in place. kyaml writes list
    /// elements back into the destination list only in this case.
    is_dest: bool,
}

impl Merged {
    fn clear() -> Self {
        Self {
            value: None,
            keep_null: false,
            is_dest: false,
        }
    }

    fn of(value: Option<Value>, is_dest: bool) -> Self {
        Self {
            value,
            keep_null: false,
            is_dest,
        }
    }

    /// What `FieldSetter` stores: nothing for a cleared or (non-persistent) null value.
    fn into_field_value(self) -> Option<Value> {
        match self.value {
            Some(Value::Null) if !self.keep_null => None,
            other => other,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Directive {
    Merge,
    Replace,
    Delete,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    Map,
    Seq,
    Scalar,
}

fn kind_of(v: &Value) -> Kind {
    match v {
        Value::Object(_) => Kind::Map,
        Value::Array(_) => Kind::Seq,
        _ => Kind::Scalar,
    }
}

fn kind_name(k: Kind) -> &'static str {
    match k {
        Kind::Map => "MappingNode",
        Kind::Seq => "SequenceNode",
        Kind::Scalar => "ScalarNode",
    }
}

fn is_missing_or_null(v: Option<&Value>) -> bool {
    v.is_none_or(Value::is_null)
}

/// A scalar's text, as kyaml reads `YNode().Value`.
fn text(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Number(n) => n.to_string(),
        Value::Bool(b) => b.to_string(),
        _ => String::new(),
    }
}

/// The schema of a node that names its own kind (`Walker.GetSchema`).
fn schema_from_meta(nodes: [Option<&Value>; 2]) -> Option<Schema> {
    nodes.into_iter().flatten().find_map(|n| {
        let kind = n.get("kind")?.as_str()?;
        let api_version = n.get("apiVersion")?.as_str()?;
        if kind.is_empty() || api_version.is_empty() {
            return None;
        }
        Schema::for_kind(api_version, kind)
    })
}

struct Walker {
    options: MergeOptions,
}

impl Walker {
    fn walk(
        &self,
        dest: Option<Value>,
        patch: Option<Value>,
        schema: Option<Schema>,
    ) -> Result<Merged> {
        let schema = schema.or_else(|| schema_from_meta([dest.as_ref(), patch.as_ref()]));
        let kind = [dest.as_ref(), patch.as_ref()]
            .into_iter()
            .flatten()
            .find(|v| !v.is_null())
            .map(kind_of);
        if let Some(kind) = kind {
            for v in [dest.as_ref(), patch.as_ref()].into_iter().flatten() {
                if !v.is_null() && kind_of(v) != kind {
                    return Err(Error::Invalid(format!(
                        "wrong node kind: expected {} but got {}",
                        kind_name(kind),
                        kind_name(kind_of(v))
                    )));
                }
            }
        }
        match kind {
            None | Some(Kind::Map) => self.walk_map(dest, patch, schema),
            Some(Kind::Seq) => {
                if self.is_associative(schema, [dest.as_ref(), patch.as_ref()]) {
                    self.walk_associative(dest, patch, schema)
                } else {
                    // VisitList, NonAssociateList: the patch list wins.
                    Ok(match patch {
                        Some(p) => Merged::of(Some(p), false),
                        None => Merged::of(dest, true),
                    })
                }
            }
            // VisitScalar: the patch value wins.
            Some(Kind::Scalar) => Ok(match patch {
                Some(p) => Merged::of(Some(p), false),
                None => Merged::of(dest, true),
            }),
        }
    }

    /// `schema.IsAssociative`: from the schema when there is one, else inferred.
    fn is_associative(&self, schema: Option<Schema>, nodes: [Option<&Value>; 2]) -> bool {
        if let Some(schema) = schema {
            return schema.is_associative();
        }
        self.options.infer_lists
            && nodes
                .into_iter()
                .flatten()
                .any(|n| associative_key(n).is_some())
    }

    fn walk_map(
        &self,
        dest: Option<Value>,
        patch: Option<Value>,
        schema: Option<Schema>,
    ) -> Result<Merged> {
        let mut patch = patch;
        // VisitMap
        let (base, is_dest) = if is_missing_or_null(dest.as_ref()) {
            if directive(&mut patch).unwrap_or(Directive::Merge) == Directive::Delete {
                return Ok(Merged::clear());
            }
            if patch.is_none() && dest.is_some() {
                // A null in the destination that the patch doesn't touch stays.
                return Ok(Merged {
                    value: Some(Value::Null),
                    keep_null: true,
                    is_dest: false,
                });
            }
            (patch.clone(), false)
        } else {
            if patch.as_ref().is_some_and(Value::is_null) {
                return Ok(Merged::clear());
            }
            match directive(&mut patch)? {
                Directive::Delete => return Ok(Merged::clear()),
                Directive::Replace => (patch.clone(), false),
                Directive::Merge => (dest, true),
            }
        };
        let Some(Value::Object(mut out)) = base else {
            return Ok(Merged::of(base, is_dest));
        };
        // walkMap: every field of either side, in sorted order; new fields are
        // appended, existing ones updated in place.
        let patch_map = match &patch {
            Some(Value::Object(m)) => Some(m),
            _ => None,
        };
        let fields: BTreeSet<String> = out
            .keys()
            .chain(patch_map.into_iter().flat_map(Map::keys))
            .cloned()
            .collect();
        for key in fields {
            let field_schema = schema.and_then(|s| s.field(&key));
            let child = self.walk(
                out.get(&key).cloned(),
                patch_map.and_then(|m| m.get(&key)).cloned(),
                field_schema,
            )?;
            match child.into_field_value() {
                Some(v) => {
                    out.insert(key, v);
                }
                None => {
                    out.shift_remove(&key);
                }
            }
        }
        Ok(Merged::of(Some(Value::Object(out)), is_dest))
    }

    fn walk_associative(
        &self,
        dest: Option<Value>,
        patch: Option<Value>,
        schema: Option<Schema>,
    ) -> Result<Merged> {
        let mut patch = patch;
        // VisitList, AssociativeList
        let base = if is_missing_or_null(dest.as_ref()) {
            patch.clone()
        } else {
            if patch.as_ref().is_some_and(Value::is_null) {
                return Ok(Merged::clear());
            }
            match directive(&mut patch)? {
                Directive::Delete => return Ok(Merged::clear()),
                Directive::Replace => patch.clone(),
                Directive::Merge => dest,
            }
        };
        let Some(Value::Array(dest_list)) = base else {
            return Ok(Merged::of(base, false));
        };
        let patch_list: Vec<Value> = match &patch {
            Some(Value::Array(items)) => items.clone(),
            _ => Vec::new(),
        };
        let has_patch = matches!(patch, Some(Value::Array(_)));

        let mut keys: Vec<String> = schema.map(|s| s.merge_keys().to_vec()).unwrap_or_default();
        let strategy = schema.map(|s| s.patch_strategy()).unwrap_or_default();
        if strategy.is_empty() && keys.is_empty() {
            // No schema: infer the key from the elements (only reachable when
            // inference is on).
            let mut inferred: Option<&str> = None;
            for list in [Some(&dest_list), has_patch.then_some(&patch_list)]
                .into_iter()
                .flatten()
            {
                if list.is_empty() {
                    continue;
                }
                let key = list_associative_key(list).unwrap_or_default();
                if let Some(prev) = inferred
                    && prev != key
                {
                    return Err(Error::Invalid(format!(
                        "conflicting merge keys [{prev},{key}]"
                    )));
                }
                inferred = Some(key);
            }
            match inferred {
                Some(k) if !k.is_empty() => keys.push(k.to_string()),
                _ => return Err(Error::Invalid("no merge key found".to_string())),
            }
        }

        let sources = self.ordered([&dest_list, &patch_list], has_patch);
        let element_schema = schema.and_then(|s| s.elements());
        let values = element_values(&sources, &keys);
        // Lists of scalars (no merge keys) match elements by their own value.
        let (rows, keys) = if !values.is_empty() || !keys.is_empty() {
            (values, keys)
        } else {
            (primitive_values(&sources), vec![String::new()])
        };
        let merged = self.set_elements(
            rows,
            &keys,
            dest_list,
            &patch_list,
            has_patch,
            element_schema,
        )?;
        Ok(Merged::of(Some(Value::Array(merged)), false))
    }

    /// Sources in the order kyaml reads element values: patch first when prepending.
    fn ordered<'a>(
        &self,
        [dest, patch]: [&'a Vec<Value>; 2],
        has_patch: bool,
    ) -> Vec<&'a Vec<Value>> {
        let mut out = vec![dest];
        if has_patch {
            out.push(patch);
        }
        if self.options.prepend && out.len() == 2 {
            out.swap(0, 1);
        }
        out
    }

    /// `setAssociativeSequenceElements`.
    fn set_elements(
        &self,
        mut values_list: Vec<Vec<String>>,
        keys: &[String],
        mut dest_list: Vec<Value>,
        patch_list: &[Value],
        has_patch: bool,
        element_schema: Option<Schema>,
    ) -> Result<Vec<Value>> {
        let mut items: Vec<Value> = Vec::new();
        if keys.len() > 1 {
            merge_values(&mut values_list);
        }
        let mut valid_keys: Vec<String> = Vec::new();
        for values in &values_list {
            if values.is_empty() {
                continue;
            }
            let (vk, vv) = validate_keys(&values_list, values, keys);
            valid_keys = vk;
            let valid_values = vv;
            // elementValueList validates again against this row only.
            let (mk, mv) = validate_keys(
                std::slice::from_ref(&valid_values),
                &valid_values,
                &valid_keys,
            );
            let dest_index = match_element(&dest_list, &mk, &mv);
            let patch_elem = if has_patch {
                match_element(patch_list, &mk, &mv).map(|i| patch_list[i].clone())
            } else {
                None
            };
            let merged = self.walk(
                dest_index.map(|i| dest_list[i].clone()),
                patch_elem,
                element_schema,
            )?;
            if merged.is_dest
                && let (Some(i), Some(v)) = (dest_index, &merged.value)
            {
                dest_list[i] = v.clone();
            }
            let mut val = merged.value;
            let mut exit = false;
            for (i, key) in valid_keys.iter().enumerate() {
                let empty = match &val {
                    None | Some(Value::Null) => true,
                    Some(Value::Object(m)) => m.is_empty(),
                    _ => false,
                };
                if empty {
                    element_setter(&mut dest_list, None, &valid_keys, &valid_values)?;
                    exit = true;
                } else if let Some(v) = val.as_mut() {
                    let has_field = v.as_object().is_some_and(|m| m.contains_key(key));
                    if !has_field && !valid_values[i].is_empty() {
                        if key.is_empty() {
                            replace_scalar(v, &valid_values[i], Tag::Untagged)?;
                        } else {
                            set_map_entry(v, key, &valid_values[i], Tag::Untagged)?;
                        }
                    }
                }
            }
            if exit {
                continue;
            }
            element_setter(&mut items, val, &valid_keys, &valid_values)?;
        }
        if values_list.is_empty() {
            return Ok(dest_list);
        }
        if self.options.prepend {
            append_list_node(items, dest_list, &valid_keys)
        } else {
            append_list_node(dest_list, items, &valid_keys)
        }
    }
}

/// `determineSmpDirective`, removing a recognised directive from the patch.
fn directive(patch: &mut Option<Value>) -> Result<Directive> {
    let parse = |v: &str| match v {
        "delete" => Some(Directive::Delete),
        "replace" => Some(Directive::Replace),
        "merge" => Some(Directive::Merge),
        _ => None,
    };
    match patch {
        None => Ok(Directive::Merge),
        Some(Value::Object(map)) => {
            let Some(value) = map.get(DIRECTIVE) else {
                return Ok(Directive::Merge);
            };
            let found = parse(&text(value));
            match found {
                Some(d) => {
                    map.shift_remove(DIRECTIVE);
                    Ok(d)
                }
                None => Err(Error::Invalid(format!(
                    "unknown patch strategy {:?}",
                    text(value)
                ))),
            }
        }
        Some(Value::Array(items)) => {
            // The first element that is a map with a `$patch` key.
            let Some(elem) = items
                .iter()
                .find(|e| e.as_object().is_some_and(|m| m.contains_key(DIRECTIVE)))
            else {
                return Ok(Directive::Merge);
            };
            let Some(map) = elem.as_object() else {
                return Ok(Directive::Merge);
            };
            if map.len() > 1 {
                return Ok(Directive::Merge);
            }
            let value = text(&map[DIRECTIVE]);
            match parse(&value) {
                Some(d) => {
                    // elideSequencePatchDirective
                    element_setter(
                        items,
                        None,
                        &[DIRECTIVE.to_string()],
                        std::slice::from_ref(&value),
                    )?;
                    Ok(d)
                }
                None => Err(Error::Invalid(format!("unknown patch strategy {value:?}"))),
            }
        }
        Some(other) => Err(Error::Invalid(format!(
            "invalid patch directive target: {other}"
        ))),
    }
}

/// `GetAssociativeKey` for a list: a key every element has.
fn list_associative_key(list: &[Value]) -> Option<&'static str> {
    INFERRED_KEYS
        .into_iter()
        .find(|k| list.iter().all(|e| e.get(k).is_some()))
}

fn associative_key(node: &Value) -> Option<&'static str> {
    node.as_array().and_then(|l| list_associative_key(l))
}

/// `elementValues`: each element's merge-key values, de-duplicated, in source order.
fn element_values(sources: &[&Vec<Value>], keys: &[String]) -> Vec<Vec<String>> {
    let mut out: Vec<Vec<String>> = Vec::new();
    for list in sources {
        for elem in list.iter() {
            let row: Vec<String> = keys
                .iter()
                .map(|k| match elem.get(k) {
                    None | Some(Value::Null) => String::new(),
                    Some(Value::Object(m)) if m.is_empty() => String::new(),
                    Some(Value::Array(a)) if a.is_empty() => String::new(),
                    Some(v) => text(v),
                })
                .collect();
            if row.is_empty() || out.contains(&row) {
                continue;
            }
            out.push(row);
        }
    }
    out
}

/// `elementPrimitiveValues`: each element's own value, de-duplicated.
fn primitive_values(sources: &[&Vec<Value>]) -> Vec<Vec<String>> {
    let mut seen = BTreeSet::new();
    let mut out = Vec::new();
    for list in sources {
        for item in list.iter() {
            let t = text(item);
            if seen.insert(t.clone()) {
                out.push(vec![t]);
            }
        }
    }
    out
}

/// `validateKeys`: drop keys no element sets, unless none are set at all.
fn validate_keys(
    values_list: &[Vec<String>],
    values: &[String],
    keys: &[String],
) -> (Vec<String>, Vec<String>) {
    let valid: BTreeSet<&String> = values_list
        .iter()
        .flat_map(|row| {
            row.iter()
                .enumerate()
                .filter(|(_, v)| !v.is_empty())
                .map(|(i, _)| i)
        })
        .filter_map(|i| keys.get(i))
        .collect();
    if valid.is_empty() {
        return (keys.to_vec(), values.to_vec());
    }
    let valid_keys = keys.iter().filter(|k| valid.contains(k)).cloned().collect();
    let valid_values = values
        .iter()
        .enumerate()
        .filter(|(i, v)| !v.is_empty() || keys.get(*i).is_some_and(|k| valid.contains(k)))
        .map(|(_, v)| v.clone())
        .collect();
    (valid_keys, valid_values)
}

/// `mergeValues`: rows that agree on every set key are unified.
fn merge_values(values_list: &mut [Vec<String>]) {
    for i in 0..values_list.len() {
        let values1 = values_list[i].clone();
        for j in 0..values_list.len() {
            let values2 = values_list[j].clone();
            if let Some(merged) = match_rows(&values1, &values2) {
                values_list[i] = merged.clone();
                values_list[j] = merged;
            }
        }
    }
}

fn match_rows(a: &[String], b: &[String]) -> Option<Vec<String>> {
    if a.len() != b.len() {
        return None;
    }
    let mut common = false;
    let mut out = Vec::with_capacity(a.len());
    for (x, y) in a.iter().zip(b) {
        if x == y {
            common = true;
            out.push(x.clone());
        } else if !x.is_empty() && !y.is_empty() {
            return None;
        } else if !x.is_empty() {
            out.push(x.clone());
        } else {
            out.push(y.clone());
        }
    }
    common.then_some(out)
}

/// `MatchElementList`: the first element matching `keys` = `values`.
fn match_element(list: &[Value], keys: &[String], values: &[String]) -> Option<usize> {
    let first_value = values.first().cloned().unwrap_or_default();
    if keys.first().is_none_or(String::is_empty) {
        return list.iter().position(|e| text(e) == first_value);
    }
    if keys.len() != values.len() {
        return None;
    }
    list.iter().position(|e| {
        e.as_object().is_some_and(|m| {
            keys.iter()
                .zip(values)
                .all(|(k, v)| m.get(k).is_some_and(|f| text(f) == *v))
        })
    })
}

/// `FieldMatcher{Name, StringValue}` on one element.
fn field_matches(elem: &Value, name: &str, value: &str) -> Result<bool> {
    if elem.is_null() {
        return Ok(false);
    }
    if name.is_empty() {
        if elem.is_object() || elem.is_array() {
            return Err(Error::Invalid(
                "wrong node kind: expected ScalarNode".to_string(),
            ));
        }
        return Ok(text(elem) == value);
    }
    let Some(map) = elem.as_object() else {
        return Err(Error::Invalid(
            "wrong node kind: expected MappingNode".to_string(),
        ));
    };
    Ok(match map.get(name) {
        None => false,
        // An empty value matches any element that has the field.
        Some(_) if value.is_empty() => true,
        Some(f) => text(f) == value,
    })
}

/// `ElementSetter`: replace (or, with `None`, remove) every element matching
/// `keys` = `values`; append the element if none matched. Null and empty-map
/// elements are dropped along the way, as upstream does.
fn element_setter(
    list: &mut Vec<Value>,
    element: Option<Value>,
    keys: &[String],
    values: &[String],
) -> Result<()> {
    let keys: Vec<String> = if keys.is_empty() {
        vec![String::new()]
    } else {
        keys.to_vec()
    };
    let mapping_setter = !keys[0].is_empty() && values.first().is_some_and(|v| !v.is_empty());
    let mut out = Vec::with_capacity(list.len());
    let mut found_any = false;
    for elem in list.drain(..) {
        if elem.is_null() || elem.as_object().is_some_and(Map::is_empty) {
            continue;
        }
        if !elem.is_object() && mapping_setter {
            out.push(elem);
            continue;
        }
        // Upstream keeps the last match result when there are more keys than values.
        let mut last = false;
        let mut found = true;
        for (j, key) in keys.iter().enumerate() {
            if let Some(v) = values.get(j) {
                last = field_matches(&elem, key, v)?;
            }
            if !last {
                found = false;
                break;
            }
        }
        if !found {
            if !values.is_empty() {
                out.push(elem);
            }
            continue;
        }
        found_any = true;
        if let Some(e) = &element {
            out.push(e.clone());
        }
    }
    *list = out;
    match element {
        None | Some(Value::Null) => Ok(()),
        Some(e) => {
            if !found_any {
                list.push(e);
            }
            Ok(())
        }
    }
}

/// `appendListNode`: set each element of `src` into `dst` by its key values.
fn append_list_node(mut dst: Vec<Value>, src: Vec<Value>, keys: &[String]) -> Result<Vec<Value>> {
    let keys: Vec<String> = if keys.is_empty() {
        vec![String::new()]
    } else {
        keys.to_vec()
    };
    for elem in src {
        if keys[0].is_empty() {
            let v = text(&elem);
            element_setter(&mut dst, Some(elem), &[String::new()], &[v])?;
            continue;
        }
        let mut v = Vec::new();
        for key in &keys {
            let Some(map) = elem.as_object() else {
                return Err(Error::Invalid(
                    "wrong node kind: expected MappingNode".to_string(),
                ));
            };
            match map.get(key) {
                None => dst.push(elem.clone()),
                Some(field) => v.push(text(field)),
            }
        }
        if keys.len() > 1 {
            element_setter(&mut dst, None, &keys, &v)?;
        }
        element_setter(&mut dst, Some(elem), &keys, &v)?;
    }
    Ok(dst)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn kustomize_merge(dest: Value, patch: Value) -> Option<Value> {
        merge(&dest, &patch, MergeOptions::kustomize()).unwrap()
    }

    fn deployment(containers: Value) -> Value {
        json!({"apiVersion": "apps/v1", "kind": "Deployment", "metadata": {"name": "d"},
               "spec": {"template": {"spec": {"containers": containers}}}})
    }

    #[test]
    fn maps_merge_and_null_deletes() {
        let out = kustomize_merge(
            json!({"kind": "X", "a": 1, "b": {"c": 2, "d": 3}}),
            json!({"kind": "X", "a": null, "b": {"c": 9}, "e": 5}),
        )
        .unwrap();
        assert_eq!(out, json!({"kind": "X", "b": {"c": 9, "d": 3}, "e": 5}));
    }

    #[test]
    fn containers_merge_by_name_with_patch_order_first() {
        let out = kustomize_merge(
            deployment(json!([{"name": "a", "image": "a:1"}, {"name": "b", "image": "b:1"}])),
            deployment(json!([{"name": "b", "image": "b:2"}, {"name": "c", "image": "c:1"}])),
        )
        .unwrap();
        assert_eq!(
            out["spec"]["template"]["spec"]["containers"],
            json!([{"name": "b", "image": "b:2"}, {"name": "c", "image": "c:1"}, {"name": "a", "image": "a:1"}])
        );
    }

    #[test]
    fn lists_without_a_merge_strategy_are_replaced() {
        let dest = deployment(json!([{"name": "a", "args": ["x", "y"]}]));
        let patch = deployment(json!([{"name": "a", "args": ["z"]}]));
        let out = kustomize_merge(dest, patch).unwrap();
        assert_eq!(
            out["spec"]["template"]["spec"]["containers"][0]["args"],
            json!(["z"])
        );
        // Unknown kinds have no schema: their lists are replaced too.
        let out = kustomize_merge(
            json!({"apiVersion": "x.io/v1", "kind": "W", "items": [{"name": "a"}, {"name": "b"}]}),
            json!({"apiVersion": "x.io/v1", "kind": "W", "items": [{"name": "b", "v": 1}]}),
        )
        .unwrap();
        assert_eq!(out["items"], json!([{"name": "b", "v": 1}]));
    }

    #[test]
    fn patch_delete_directive_on_an_element_and_on_the_resource() {
        let out = kustomize_merge(
            deployment(json!([{"name": "a"}, {"name": "b"}])),
            deployment(json!([{"name": "a", "$patch": "delete"}])),
        )
        .unwrap();
        assert_eq!(
            out["spec"]["template"]["spec"]["containers"],
            json!([{"name": "b"}])
        );
        assert!(
            kustomize_merge(
                json!({"kind": "X"}),
                json!({"kind": "X", "$patch": "delete"})
            )
            .is_none()
        );
    }

    #[test]
    fn patch_replace_directive_on_a_map() {
        let out = kustomize_merge(
            json!({"kind": "X", "m": {"a": 1, "b": 2}}),
            json!({"kind": "X", "m": {"$patch": "replace", "c": 3}}),
        )
        .unwrap();
        assert_eq!(out["m"], json!({"c": 3}));
    }

    #[test]
    fn other_directives_pass_through_like_kyaml() {
        let out = kustomize_merge(
            json!({"kind": "X", "m": {"a": 1}}),
            json!({"kind": "X", "m": {"$retainKeys": ["a"]}}),
        )
        .unwrap();
        assert_eq!(out["m"], json!({"a": 1, "$retainKeys": ["a"]}));
    }

    #[test]
    fn primitive_merge_lists_union_by_value() {
        let meta = |f: Value| json!({"apiVersion": "v1", "kind": "ConfigMap", "metadata": {"name": "c", "finalizers": f}});
        let out = kustomize_merge(meta(json!(["a", "b"])), meta(json!(["b", "c"]))).unwrap();
        assert_eq!(out["metadata"]["finalizers"], json!(["b", "c", "a"]));
    }

    #[test]
    fn mismatched_kinds_are_an_error() {
        assert!(
            merge(
                &json!({"kind": "X", "a": {"b": 1}}),
                &json!({"kind": "X", "a": "s"}),
                MergeOptions::kustomize()
            )
            .is_err()
        );
    }

    #[test]
    fn unknown_directive_is_an_error() {
        assert!(
            merge(
                &json!({"kind": "X", "m": {"a": 1}}),
                &json!({"kind": "X", "m": {"$patch": "bogus"}}),
                MergeOptions::kustomize()
            )
            .is_err()
        );
    }

    /// The normalization kyaml's test harness applies before comparing
    /// (`filters.FormatInput`): containers of workload kinds are sorted by
    /// name, webhook rule operations by value. Map order doesn't matter here.
    fn format_input(mut v: Value) -> Value {
        let kind = v["kind"].as_str().unwrap_or_default().to_string();
        let api = v["apiVersion"].as_str().unwrap_or_default().to_string();
        let kinds = [
            "CronJob",
            "DaemonSet",
            "Deployment",
            "Job",
            "ReplicaSet",
            "StatefulSet",
            "ValidatingWebhookConfiguration",
        ];
        let apis = [
            "apps/v1",
            "apps/v1beta1",
            "apps/v1beta2",
            "batch/v1",
            "batch/v1beta1",
            "extensions/v1beta1",
            "v1",
            "admissionregistration.k8s.io/v1",
        ];
        if kinds.contains(&kind.as_str())
            && apis.contains(&api.as_str())
            && let Some(Value::Array(c)) = v.pointer_mut("/spec/template/spec/containers")
        {
            c.sort_by_key(|e| text(&e["name"]));
        }
        v
    }

    /// kyaml's own merge2 tables, extracted to tests/fixtures/units/merge2.json.
    /// Two cases declare their schema in a YAML comment (`# {"type": ...}`),
    /// which kyaml reads but `serde_json::Value` can't carry; they're skipped.
    #[test]
    fn upstream_merge2_tables() {
        let doc: Value =
            serde_json::from_str(include_str!("../tests/fixtures/units/merge2.json")).unwrap();
        let mut failures = Vec::new();
        let cases = doc["cases"].as_array().unwrap();
        for case in cases {
            let name = case["description"].as_str().unwrap();
            if ["source", "dest"]
                .iter()
                .any(|k| case[*k].as_str().unwrap().contains("# {"))
            {
                continue;
            }
            let parse = |k: &str| -> Value {
                crate::yaml::documents(case[k].as_str().unwrap())
                    .unwrap()
                    .into_iter()
                    .next()
                    .unwrap_or(Value::Null)
            };
            let options = MergeOptions {
                prepend: case["prepend"].as_bool().unwrap(),
                infer_lists: case["infer"].as_bool().unwrap(),
            };
            match merge(&parse("dest"), &parse("source"), options) {
                Ok(out) => {
                    let want = format_input(parse("expected"));
                    let got = format_input(out.unwrap_or(Value::Null));
                    if got != want {
                        failures.push(format!("{name}:\n  want {want}\n  got  {got}"));
                    }
                }
                Err(e) => failures.push(format!("{name}: error {e}")),
            }
        }
        assert!(
            failures.is_empty(),
            "{} of {} upstream merge2 cases failed:\n{}",
            failures.len(),
            cases.len(),
            failures.join("\n")
        );
    }
}
