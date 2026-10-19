//! Tagged encoding of non-JSON TypeScript values (parity/lib.ts `encode`): constructors for building expected /
//! actual values, `JsValue` for an exact decoded view, and `untag` for feeding serde.
//!
//! A tag is an object with exactly one `$`-key: `{"$u":1}` undefined, `{"$num":"NaN"|"Infinity"|"-Infinity"}`,
//! `{"$bigint":"12"}`, `{"$date":"<iso>"|null}`, `{"$map":[[k,v],..]}`, `{"$set":[..]}`, `{"$fn":"name"}`,
//! `{"$screen":{..}}`, `{"$bytes":"<base64>"}`, `{"$error":{"name","message"[,"code"]}}`, `{"$regexp":"/src/flags"}`,
//! `{"$utf16":[units]}` (string with lone surrogates), `{"$symbol":"desc"}`, `{"$cycle":1}`, and `{"$obj":{..}}`
//! (a real object whose only key looks like a tag).

use indexmap::IndexMap;
use serde_json::{Map, Value, json};

pub const TAGS: &[&str] = &[
    "$u", "$num", "$bigint", "$date", "$map", "$set", "$fn", "$screen", "$bytes", "$error", "$regexp", "$utf16",
    "$symbol", "$cycle", "$obj",
];

/// `(tag, body)` when `value` is a tag.
pub fn tag_of(value: &Value) -> Option<(&str, &Value)> {
    let object = value.as_object()?;
    if object.len() != 1 {
        return None;
    }
    let (key, body) = object.iter().next()?;
    TAGS.contains(&key.as_str()).then_some((key.as_str(), body))
}

pub fn undefined() -> Value {
    json!({"$u": 1})
}
pub fn is_undefined(value: &Value) -> bool {
    matches!(tag_of(value), Some(("$u", _)))
}
pub fn nan() -> Value {
    json!({"$num": "NaN"})
}
pub fn infinity(positive: bool) -> Value {
    json!({"$num": if positive { "Infinity" } else { "-Infinity" }})
}
/// A JS number: integral values within ±2^53 as integers (as JSON.stringify writes them), NaN/±Infinity tagged.
pub fn number(value: f64) -> Value {
    if value.is_nan() {
        nan()
    } else if value.is_infinite() {
        infinity(value > 0.0)
    } else if value.fract() == 0.0 && value.abs() <= 9_007_199_254_740_992.0 {
        json!(value as i64)
    } else {
        json!(value)
    }
}
pub fn bigint(digits: &str) -> Value {
    json!({"$bigint": digits})
}
/// `iso` as `Date.prototype.toISOString` writes it (`2026-01-01T00:00:00.000Z`); `None` = Invalid Date.
pub fn date(iso: Option<&str>) -> Value {
    json!({"$date": iso})
}
pub fn map(entries: impl IntoIterator<Item = (Value, Value)>) -> Value {
    json!({"$map": entries.into_iter().map(|(key, value)| json!([key, value])).collect::<Vec<_>>()})
}
pub fn set(items: impl IntoIterator<Item = Value>) -> Value {
    json!({"$set": items.into_iter().collect::<Vec<_>>()})
}
pub fn fn_ref(name: &str) -> Value {
    json!({"$fn": name})
}
pub fn screen(snapshot: Value) -> Value {
    json!({"$screen": snapshot})
}
pub fn bytes(data: &[u8]) -> Value {
    json!({"$bytes": base64_encode(data)})
}
pub fn error(name: &str, message: &str) -> Value {
    json!({"$error": {"name": name, "message": message}})
}

/// Exact decoded view of a tagged value; `Undefined` is distinct from `Null`.
#[derive(Debug, Clone, PartialEq)]
pub enum JsValue {
    Undefined,
    Null,
    Bool(bool),
    Number(f64),
    String(String),
    /// A string with lone surrogates (not representable as a Rust `String`).
    Utf16(Vec<u16>),
    BigInt(String),
    Date(Option<String>),
    Array(Vec<JsValue>),
    Object(IndexMap<String, JsValue>),
    Map(Vec<(JsValue, JsValue)>),
    Set(Vec<JsValue>),
    Fn(String),
    Screen(Value),
    Bytes(Vec<u8>),
    Error {
        name: String,
        message: String,
        code: Option<Box<JsValue>>,
    },
    RegExp(String),
    Symbol(String),
    Cycle,
}

fn as_str<'a>(tag: &str, body: &'a Value) -> Result<&'a str, String> {
    body.as_str()
        .ok_or_else(|| format!("{tag}: expected a string, got {body}"))
}
fn as_array<'a>(tag: &str, body: &'a Value) -> Result<&'a Vec<Value>, String> {
    body.as_array()
        .ok_or_else(|| format!("{tag}: expected an array, got {body}"))
}

pub fn decode(value: &Value) -> Result<JsValue, String> {
    if let Some((tag, body)) = tag_of(value) {
        return Ok(match tag {
            "$u" => JsValue::Undefined,
            "$num" => JsValue::Number(match as_str(tag, body)? {
                "NaN" => f64::NAN,
                "Infinity" => f64::INFINITY,
                "-Infinity" => f64::NEG_INFINITY,
                other => other.parse().map_err(|_| format!("$num: bad number {other:?}"))?,
            }),
            "$bigint" => JsValue::BigInt(as_str(tag, body)?.to_string()),
            "$date" => JsValue::Date(body.as_str().map(str::to_string)),
            "$map" => JsValue::Map(
                as_array(tag, body)?
                    .iter()
                    .map(|pair| match pair.as_array().map(Vec::as_slice) {
                        Some([key, value]) => Ok((decode(key)?, decode(value)?)),
                        _ => Err(format!("$map: expected [key, value], got {pair}")),
                    })
                    .collect::<Result<_, String>>()?,
            ),
            "$set" => JsValue::Set(as_array(tag, body)?.iter().map(decode).collect::<Result<_, _>>()?),
            "$fn" => JsValue::Fn(as_str(tag, body)?.to_string()),
            "$screen" => JsValue::Screen(body.clone()),
            "$bytes" => JsValue::Bytes(base64_decode(as_str(tag, body)?)?),
            "$error" => JsValue::Error {
                name: body.get("name").and_then(Value::as_str).unwrap_or("Error").to_string(),
                message: body
                    .get("message")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
                code: body.get("code").map(decode).transpose()?.map(Box::new),
            },
            "$regexp" => JsValue::RegExp(as_str(tag, body)?.to_string()),
            "$utf16" => JsValue::Utf16(
                as_array(tag, body)?
                    .iter()
                    .map(|unit| {
                        unit.as_u64()
                            .and_then(|unit| u16::try_from(unit).ok())
                            .ok_or("$utf16: bad unit")
                    })
                    .collect::<Result<_, _>>()?,
            ),
            "$symbol" => JsValue::Symbol(as_str(tag, body)?.to_string()),
            "$cycle" => JsValue::Cycle,
            // "$obj": an escaped plain object
            _ => decode_object(body.as_object().ok_or("$obj: expected an object")?)?,
        });
    }
    Ok(match value {
        Value::Null => JsValue::Null,
        Value::Bool(flag) => JsValue::Bool(*flag),
        Value::Number(number) => JsValue::Number(number.as_f64().unwrap_or(f64::NAN)),
        Value::String(text) => JsValue::String(text.clone()),
        Value::Array(items) => JsValue::Array(items.iter().map(decode).collect::<Result<_, _>>()?),
        Value::Object(object) => decode_object(object)?,
    })
}
fn decode_object(object: &Map<String, Value>) -> Result<JsValue, String> {
    Ok(JsValue::Object(
        object
            .iter()
            .map(|(key, value)| Ok((key.clone(), decode(value)?)))
            .collect::<Result<_, String>>()?,
    ))
}

pub fn encode(value: &JsValue) -> Value {
    match value {
        JsValue::Undefined => undefined(),
        JsValue::Null => Value::Null,
        JsValue::Bool(flag) => Value::Bool(*flag),
        JsValue::Number(number) => self::number(*number),
        JsValue::String(text) => Value::String(text.clone()),
        JsValue::Utf16(units) => json!({"$utf16": units}),
        JsValue::BigInt(digits) => bigint(digits),
        JsValue::Date(iso) => date(iso.as_deref()),
        JsValue::Array(items) => Value::Array(items.iter().map(encode).collect()),
        JsValue::Object(object) => {
            let out: Map<String, Value> = object.iter().map(|(key, value)| (key.clone(), encode(value))).collect();
            let escaped = out.len() == 1 && out.keys().all(|key| TAGS.contains(&key.as_str()));
            if escaped {
                json!({"$obj": out})
            } else {
                Value::Object(out)
            }
        }
        JsValue::Map(entries) => map(entries.iter().map(|(key, value)| (encode(key), encode(value)))),
        JsValue::Set(items) => set(items.iter().map(encode)),
        JsValue::Fn(name) => fn_ref(name),
        JsValue::Screen(snapshot) => screen(snapshot.clone()),
        JsValue::Bytes(data) => bytes(data),
        JsValue::Error { name, message, code } => {
            let mut body = json!({"name": name, "message": message});
            if let (Some(code), Some(object)) = (code, body.as_object_mut()) {
                object.insert("code".into(), encode(code));
            }
            json!({"$error": body})
        }
        JsValue::RegExp(source) => json!({"$regexp": source}),
        JsValue::Symbol(description) => json!({"$symbol": description}),
        JsValue::Cycle => json!({"$cycle": 1}),
    }
}

/// Plain JSON for serde: undefined object properties are dropped (undefined array items become null), dates and
/// bigints become strings, a `$map` with string keys becomes an object (else an array of `[k, v]`), sets become
/// arrays, `$fn` its name, `$screen` its snapshot, NaN/Infinity null, `$utf16` a lossy string, `$error` its body.
pub fn untag(value: &Value) -> Value {
    if let Some((tag, body)) = tag_of(value) {
        return match tag {
            "$u" | "$cycle" => Value::Null,
            // NaN / ±Infinity have no JSON form (JSON.stringify writes null too)
            "$num" => Value::Null,
            "$bigint" | "$date" | "$fn" | "$regexp" | "$symbol" | "$bytes" => body.clone(),
            "$map" => {
                let pairs: Vec<(Value, Value)> = body
                    .as_array()
                    .map(|pairs| {
                        pairs
                            .iter()
                            .filter_map(|pair| match pair.as_array().map(Vec::as_slice) {
                                Some([key, value]) => Some((untag(key), untag(value))),
                                _ => None,
                            })
                            .collect()
                    })
                    .unwrap_or_default();
                if pairs.iter().all(|(key, _)| key.is_string()) {
                    Value::Object(
                        pairs
                            .into_iter()
                            .filter_map(|(key, value)| Some((key.as_str()?.to_string(), value)))
                            .collect(),
                    )
                } else {
                    Value::Array(pairs.into_iter().map(|(key, value)| json!([key, value])).collect())
                }
            }
            "$set" => Value::Array(
                body.as_array()
                    .map(|items| items.iter().map(untag).collect())
                    .unwrap_or_default(),
            ),
            "$utf16" => {
                let units: Vec<u16> = body
                    .as_array()
                    .map(|units| {
                        units
                            .iter()
                            .filter_map(|unit| unit.as_u64())
                            .filter_map(|unit| u16::try_from(unit).ok())
                            .collect()
                    })
                    .unwrap_or_default();
                Value::String(String::from_utf16_lossy(&units))
            }
            // "$screen" | "$error" | "$obj"
            _ => untag(body),
        };
    }
    match value {
        Value::Array(items) => Value::Array(items.iter().map(untag).collect()),
        Value::Object(object) => Value::Object(
            object
                .iter()
                .filter(|(_, value)| !is_undefined(value))
                .map(|(key, value)| (key.clone(), untag(value)))
                .collect(),
        ),
        other => other.clone(),
    }
}

const BASE64: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

pub fn base64_encode(data: &[u8]) -> String {
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for chunk in data.chunks(3) {
        let bits = chunk.iter().enumerate().fold(0u32, |bits, (index, byte)| {
            bits | (u32::from(*byte) << (16 - 8 * index))
        });
        for index in 0..4 {
            if index <= chunk.len() {
                out.push(char::from(BASE64[((bits >> (18 - 6 * index)) & 63) as usize]));
            } else {
                out.push('=');
            }
        }
    }
    out
}

pub fn base64_decode(text: &str) -> Result<Vec<u8>, String> {
    let mut out = Vec::with_capacity(text.len() / 4 * 3);
    let (mut bits, mut count) = (0u32, 0u32);
    for character in text.bytes().filter(|byte| *byte != b'=') {
        let value = BASE64
            .iter()
            .position(|candidate| *candidate == character)
            .ok_or_else(|| format!("bad base64 byte {character}"))?;
        bits = (bits << 6) | value as u32;
        count += 6;
        if count >= 8 {
            count -= 8;
            out.push((bits >> count) as u8);
            bits &= (1 << count) - 1;
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decode_distinguishes_undefined_from_null_and_round_trips() {
        let value = json!([{"$u": 1}, null, {"$num": "NaN"}, {"$date": "2026-01-01T00:00:00.000Z"},
            {"$map": [["a", 1], [{"$u": 1}, [2]]]}, {"$set": [1, "x"]}, {"$fn": "isEven"},
            {"$obj": {"$u": 5}}, {"$bytes": "AAEC/w=="}, {"$utf16": [55357]}, 1.5, 7]);
        let decoded = decode(&value).unwrap();
        let JsValue::Array(items) = &decoded else {
            panic!("array")
        };
        assert_eq!(items[0], JsValue::Undefined);
        assert_eq!(items[1], JsValue::Null);
        assert!(matches!(items[2], JsValue::Number(number) if number.is_nan()));
        assert_eq!(items[3], JsValue::Date(Some("2026-01-01T00:00:00.000Z".into())));
        assert_eq!(items[6], JsValue::Fn("isEven".into()));
        assert_eq!(
            items[7],
            JsValue::Object(IndexMap::from([("$u".to_string(), JsValue::Number(5.0))]))
        );
        assert_eq!(items[8], JsValue::Bytes(vec![0, 1, 2, 255]));
        assert_eq!(items[9], JsValue::Utf16(vec![55357]));
        assert_eq!(encode(&decoded), value);
    }

    #[test]
    fn untag_makes_plain_json() {
        let value = json!({"a": {"$u": 1}, "b": [{"$u": 1}], "m": {"$map": [["k", {"$set": [1]}]]}, "d": {"$date": "x"}, "n": {"$num": "Infinity"}});
        assert_eq!(
            untag(&value),
            json!({"b": [null], "m": {"k": [1]}, "d": "x", "n": null})
        );
        assert_eq!(untag(&json!({"$map": [[1, 2]]})), json!([[1, 2]]));
    }

    #[test]
    fn numbers_encode_like_json_stringify() {
        assert_eq!(number(3.0), json!(3));
        assert_eq!(number(-0.0), json!(0));
        assert_eq!(number(0.5), json!(0.5));
        assert_eq!(number(f64::NEG_INFINITY), json!({"$num": "-Infinity"}));
    }

    #[test]
    fn base64_round_trips() {
        for data in [
            &b""[..],
            b"f",
            b"fo",
            b"foo",
            b"foob",
            b"fooba",
            b"foobar",
            &[0, 255, 128],
        ] {
            assert_eq!(base64_decode(&base64_encode(data)).unwrap(), data);
        }
        assert_eq!(base64_encode(b"foobar"), "Zm9vYmFy");
        assert_eq!(base64_encode(b"fo"), "Zm8=");
    }
}
