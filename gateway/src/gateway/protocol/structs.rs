//! Conversion between plain JSON values and google.protobuf.Struct as protobufjs represents it (and the codec's
//! plain objects do): `{fields: {key: {stringValue: …}}}`.

use serde_json::{json, Map, Value};

fn to_value(value: &Value) -> Value {
    match value {
        Value::Null => json!({ "nullValue": 0 }),
        Value::Number(number) => json!({ "numberValue": number }),
        Value::String(text) => json!({ "stringValue": text }),
        Value::Bool(flag) => json!({ "boolValue": flag }),
        Value::Array(items) => json!({ "listValue": { "values": items.iter().map(to_value).collect::<Vec<_>>() } }),
        Value::Object(object) => json!({ "structValue": to_struct(object) }),
    }
}

pub fn to_struct(object: &Map<String, Value>) -> Value {
    let fields: Map<String, Value> = object.iter().map(|(key, value)| (key.clone(), to_value(value))).collect();
    json!({ "fields": fields })
}

/// A Value as plain JSON; None where the TS had `undefined` (an empty struct).
fn from_value(value: &Value) -> Option<Value> {
    let Value::Object(kind) = value else { return Some(Value::Null) };
    if let Some(text) = kind.get("stringValue") {
        return Some(text.clone());
    }
    if let Some(number) = kind.get("numberValue") {
        return Some(number.clone());
    }
    if let Some(flag) = kind.get("boolValue") {
        return Some(flag.clone());
    }
    if let Some(object) = kind.get("structValue") {
        return from_struct(Some(object)).map(Value::Object);
    }
    if let Some(list) = kind.get("listValue") {
        let values = list.get("values").and_then(Value::as_array).map(Vec::as_slice).unwrap_or_default();
        return Some(Value::Array(values.iter().map(|item| from_value(item).unwrap_or(Value::Null)).collect()));
    }
    Some(Value::Null)
}

/// The struct's fields as a JSON object; None when there are none (a struct with no fields came as `{}`).
pub fn from_struct(r#struct: Option<&Value>) -> Option<Map<String, Value>> {
    let fields = r#struct?.get("fields")?.as_object()?;
    // A key whose value was `undefined` in the TS (an empty nested struct) is left out, as JSON left it out.
    Some(fields.iter().filter_map(|(key, value)| from_value(value).map(|plain| (key.clone(), plain))).collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn converts_plain_values_to_a_struct_and_back() {
        let plain =
            json!({ "text": "привет", "n": 1.5, "on": true, "none": null, "list": [1, "a"], "nested": { "k": "v" } });
        let object = plain.as_object().unwrap();
        let r#struct = to_struct(object);
        assert_eq!(r#struct["fields"]["text"], json!({ "stringValue": "привет" }));
        assert_eq!(r#struct["fields"]["none"], json!({ "nullValue": 0 }));
        assert_eq!(
            r#struct["fields"]["list"],
            json!({ "listValue": { "values": [{ "numberValue": 1 }, { "stringValue": "a" }] } })
        );
        assert_eq!(
            r#struct["fields"]["nested"],
            json!({ "structValue": { "fields": { "k": { "stringValue": "v" } } } })
        );
        assert_eq!(from_struct(Some(&r#struct)).as_ref(), Some(object));
    }

    #[test]
    fn reads_a_struct_as_protobufjs_decoded_it() {
        // Decoded: no `fields` for an empty struct, the null value by its enum name.
        assert_eq!(from_struct(Some(&json!({}))), None);
        assert_eq!(from_struct(None), None);
        let decoded = json!({ "fields": {
            "gone": { "structValue": {} },
            "null": { "nullValue": "NULL_VALUE" },
            "empty": { "listValue": {} },
            "holes": { "listValue": { "values": [{ "structValue": {} }] } }
        } });
        assert_eq!(
            from_struct(Some(&decoded)).map(Value::Object),
            Some(json!({ "null": null, "empty": [], "holes": [null] }))
        );
    }
}
