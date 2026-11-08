use serde_json::{Map, Value, json};

pub fn compact(schema: &Value) -> Value {
    match schema {
        Value::Object(object) => Value::Object(compact_object(object)),
        Value::Array(items) => Value::Array(items.iter().map(compact).collect()),
        other => other.clone(),
    }
}

fn compact_object(object: &Map<String, Value>) -> Map<String, Value> {
    let mut out = Map::new();
    for (key, value) in object {
        match key.as_str() {
            "$schema" | "$id" => {}
            "additionalProperties" if value == &Value::Bool(true) => {}
            "default" if value.is_null() => {}
            "type" => {
                if let Some(kind) = drop_null_type(value) {
                    out.insert(key.clone(), kind);
                }
            }
            _ => {
                out.insert(key.clone(), compact(value));
            }
        }
    }
    for union in ["anyOf", "oneOf"] {
        if let Some(Value::Array(branches)) = out.remove(union) {
            merge_union(&mut out, union, branches);
        }
    }
    out
}

fn drop_null_type(kind: &Value) -> Option<Value> {
    let Value::Array(kinds) = kind else {
        return Some(kind.clone());
    };
    let kept: Vec<Value> = kinds
        .iter()
        .filter(|k| k.as_str() != Some("null"))
        .cloned()
        .collect();
    match kept.len() {
        0 => None,
        1 => kept.into_iter().next(),
        _ => Some(Value::Array(kept)),
    }
}

fn is_null_branch(branch: &Value) -> bool {
    branch.get("type").and_then(Value::as_str) == Some("null")
        || branch.get("const").is_some_and(Value::is_null)
}

fn merge_union(out: &mut Map<String, Value>, union: &str, branches: Vec<Value>) {
    let kept: Vec<Value> = branches.into_iter().filter(|b| !is_null_branch(b)).collect();
    if let [only] = kept.as_slice() {
        if let Value::Object(fields) = only {
            for (key, value) in fields {
                out.entry(key.clone()).or_insert_with(|| value.clone());
            }
        }
        return;
    }
    if let Some(merged) = merge_tagged(&kept) {
        for (key, value) in merged {
            out.entry(key).or_insert(value);
        }
        return;
    }
    if !kept.is_empty() {
        out.insert(union.to_owned(), Value::Array(kept));
    }
}

fn merge_tagged(branches: &[Value]) -> Option<Map<String, Value>> {
    if branches.len() < 2 {
        return None;
    }
    let objects: Vec<&Map<String, Value>> = branches
        .iter()
        .map(|b| b.get("properties").and_then(Value::as_object))
        .collect::<Option<_>>()?;
    let tag = objects[0].iter().find_map(|(name, prop)| {
        prop.get("const")?;
        objects
            .iter()
            .all(|o| o.get(name).and_then(|p| p.get("const")).is_some())
            .then(|| name.clone())
    })?;
    let mut properties = Map::new();
    let mut tags = Vec::new();
    for object in &objects {
        for (name, prop) in *object {
            if *name == tag {
                tags.push(prop["const"].clone());
            } else {
                properties.entry(name.clone()).or_insert_with(|| prop.clone());
            }
        }
    }
    properties.insert(tag.clone(), json!({ "type": "string", "enum": tags }));
    let mut merged = Map::new();
    merged.insert("type".into(), json!("object"));
    merged.insert("properties".into(), Value::Object(properties));
    merged.insert("required".into(), json!([tag]));
    Some(merged)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn noise_goes_and_meaning_stays() {
        let schema = json!({
            "$schema": "https://json-schema.org/draft/2020-12/schema",
            "type": "object",
            "additionalProperties": true,
            "properties": {
                "name": { "type": ["string", "null"], "default": null, "description": "Name." },
                "count": { "anyOf": [{ "type": "integer", "minimum": 1 }, { "type": "null" }] },
                "strict": { "type": "object", "additionalProperties": false }
            },
            "required": ["count"]
        });
        assert_eq!(
            compact(&schema),
            json!({
                "type": "object",
                "properties": {
                    "name": { "type": "string", "description": "Name." },
                    "count": { "type": "integer", "minimum": 1 },
                    "strict": { "type": "object", "additionalProperties": false }
                },
                "required": ["count"]
            })
        );
    }

    #[test]
    fn tagged_unions_flatten_into_one_object_with_an_enum_tag() {
        let schema = json!({
            "oneOf": [
                { "type": "object", "properties": { "kind": { "const": "a" }, "x": { "type": "string" } } },
                { "type": "object", "properties": { "kind": { "const": "b" }, "y": { "type": "number" } } }
            ]
        });
        let flat = compact(&schema);
        assert_eq!(flat["properties"]["kind"]["enum"], json!(["a", "b"]));
        assert!(flat["properties"]["x"].is_object() && flat["properties"]["y"].is_object());
        assert!(flat.get("oneOf").is_none());
    }
}
