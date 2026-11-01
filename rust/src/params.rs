//! Rails-style scalar and nested form parameters. JSON retains its native types.
use serde_json::{Map, Value};

pub fn parse_form(input: &[u8]) -> Result<Value, &'static str> {
    let pairs: Vec<(String, String)> =
        serde_urlencoded::from_bytes(input).map_err(|_| "invalid form encoding")?;
    let mut root = Value::Object(Map::new());
    for (key, value) in pairs {
        if key.len() > 1024 {
            return Err("parameter name too long");
        }
        let parts: Vec<_> = key
            .split('[')
            .map(|part| part.trim_end_matches(']'))
            .collect();
        if parts.len() > 16 {
            return Err("parameters nested too deeply");
        }
        insert(&mut root, &parts, Value::String(value))?;
    }
    Ok(root)
}

fn insert(target: &mut Value, parts: &[&str], value: Value) -> Result<(), &'static str> {
    let Some((part, tail)) = parts.split_first() else {
        *target = value;
        return Ok(());
    };
    if part.is_empty() {
        if target.is_null() {
            *target = Value::Array(Vec::new());
        }
        let array = target
            .as_array_mut()
            .ok_or("inconsistent parameter types")?;
        if tail.is_empty() {
            array.push(value);
        } else {
            let mut child = Value::Null;
            insert(&mut child, tail, value)?;
            array.push(child);
        }
    } else {
        if target.is_null() {
            *target = Value::Object(Map::new());
        }
        let map = target
            .as_object_mut()
            .ok_or("inconsistent parameter types")?;
        insert(
            map.entry((*part).to_owned()).or_insert(Value::Null),
            tail,
            value,
        )?;
    }
    Ok(())
}

pub fn scalar(value: Option<&Value>) -> Option<String> {
    match value? {
        Value::String(s) => Some(s.clone()),
        Value::Number(n) => Some(n.to_string()),
        Value::Bool(b) => Some(if *b { "true" } else { "false" }.into()),
        _ => None,
    }
}

pub fn present_scalar(value: Option<&Value>) -> Option<String> {
    if matches!(value, Some(Value::Bool(false))) {
        return None;
    }
    scalar(value).filter(|s| !s.trim().is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn decodes_existing_rails_form_names_and_arrays() {
        assert_eq!(
            parse_form(b"user%5Bnickname%5D=Jane+Doe&sink_ids%5B%5D=1&sink_ids%5B%5D=3").unwrap(),
            json!({"user":{"nickname":"Jane Doe"},"sink_ids":["1","3"]})
        );
    }

    #[test]
    fn rejects_ambiguous_parameter_structure() {
        assert!(parse_form(b"user=x&user%5Bnickname%5D=y").is_err());
    }
}
