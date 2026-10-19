//! Loading `fixtures/parity/<module>/<fn>.jsonl`: one case per line,
//! `{"args":[..], "argsAfter"?:[..], "async"?:true, "ret":..}` or `{.., "throws":"<Error.message>"}`.

use std::path::{Path, PathBuf};
use std::sync::LazyLock;

use indexmap::IndexMap;
use serde::de::DeserializeOwned;
use serde_json::Value;

use crate::tags::{self, tag_of};
use crate::utf16::{Direction, convert_case};

static UNDEFINED: LazyLock<Value> = LazyLock::new(tags::undefined);

#[derive(Debug, Clone, PartialEq)]
pub enum Expect {
    /// The tagged return value (`{"$u":1}` for undefined/void).
    Return(Value),
    /// The exact `Error.message`.
    Throws(String),
}

#[derive(Debug, Clone)]
pub struct Case {
    /// Line number from 0 (what allowlist `case` refers to).
    pub index: usize,
    /// Tagged arguments, UTF-16 offsets already converted to byte offsets when the manifest lists them.
    pub args: Vec<Value>,
    /// The arguments after the call, when the TS function mutated them.
    pub args_after: Option<Vec<Value>>,
    /// The TS function returned a promise (ret/throws are its settled value).
    pub is_async: bool,
    pub expect: Expect,
    /// The case object as loaded (after offset conversion).
    pub raw: Value,
    /// Set when `utf16_fields` could not be converted (the case then fails without running).
    pub conversion_error: Option<String>,
}

impl Case {
    pub fn from_value(
        index: usize,
        raw: Value,
        utf16_fields: Option<&IndexMap<String, String>>,
    ) -> Result<Self, String> {
        let mut converted = raw.clone();
        let mut conversion_error = None;
        if let Some(fields) = utf16_fields
            && let Err(error) = convert_case(&mut converted, fields, Direction::Utf16ToByte)
        {
            conversion_error = Some(error);
            converted = raw;
        }
        let object = converted
            .as_object()
            .ok_or_else(|| format!("case {index}: not an object"))?;
        let list = |key: &str| -> Result<Option<Vec<Value>>, String> {
            match object.get(key) {
                None => Ok(None),
                Some(Value::Array(items)) => Ok(Some(items.clone())),
                Some(other) => Err(format!("case {index}: {key} is not an array: {other}")),
            }
        };
        let args = list("args")?.ok_or_else(|| format!("case {index}: missing args"))?;
        let args_after = list("argsAfter")?;
        let expect = match (object.get("throws"), object.get("ret")) {
            (Some(Value::String(message)), _) => Expect::Throws(message.clone()),
            (Some(other), _) => return Err(format!("case {index}: throws is not a string: {other}")),
            (None, Some(ret)) => Expect::Return(ret.clone()),
            (None, None) => return Err(format!("case {index}: neither ret nor throws")),
        };
        let is_async = object.get("async") == Some(&Value::Bool(true));
        Ok(Self {
            index,
            args,
            args_after,
            is_async,
            expect,
            raw: converted,
            conversion_error,
        })
    }

    /// Argument `index` (`{"$u":1}` when the call passed fewer arguments).
    pub fn arg(&self, index: usize) -> &Value {
        self.args.get(index).unwrap_or(&UNDEFINED)
    }
    pub fn arg_is_undefined(&self, index: usize) -> bool {
        tags::is_undefined(self.arg(index))
    }
    /// Argument `index` deserialized with serde after `tags::untag` (undefined properties dropped, ...).
    pub fn arg_as<T: DeserializeOwned>(&self, index: usize) -> Result<T, String> {
        serde_json::from_value(tags::untag(self.arg(index)))
            .map_err(|error| format!("case {} arg {index}: {error}", self.index))
    }
    pub fn arg_str(&self, index: usize) -> Result<&str, String> {
        self.arg(index)
            .as_str()
            .ok_or_else(|| self.wrong_type(index, "a string"))
    }
    pub fn arg_i64(&self, index: usize) -> Result<i64, String> {
        let value = self.arg(index);
        value
            .as_i64()
            .or_else(|| {
                value
                    .as_f64()
                    .filter(|number| number.fract() == 0.0)
                    .map(|number| number as i64)
            })
            .ok_or_else(|| self.wrong_type(index, "an integer"))
    }
    pub fn arg_usize(&self, index: usize) -> Result<usize, String> {
        usize::try_from(self.arg_i64(index)?).map_err(|_| self.wrong_type(index, "a non-negative integer"))
    }
    pub fn arg_f64(&self, index: usize) -> Result<f64, String> {
        let value = self.arg(index);
        if let Some((tag, body)) = tag_of(value)
            && tag == "$num"
        {
            return body
                .as_str()
                .and_then(|text| text.parse().ok())
                .ok_or_else(|| self.wrong_type(index, "a number"));
        }
        value.as_f64().ok_or_else(|| self.wrong_type(index, "a number"))
    }
    pub fn arg_bool(&self, index: usize) -> Result<bool, String> {
        self.arg(index)
            .as_bool()
            .ok_or_else(|| self.wrong_type(index, "a boolean"))
    }
    /// The registry name of a function argument (`{"$fn":"name"}`).
    pub fn fn_name(&self, index: usize) -> Option<&str> {
        match tag_of(self.arg(index)) {
            Some(("$fn", Value::String(name))) => Some(name),
            _ => None,
        }
    }
    fn wrong_type(&self, index: usize, wanted: &str) -> String {
        format!(
            "case {} arg {index}: expected {wanted}, got {}",
            self.index,
            self.arg(index)
        )
    }
}

#[derive(Debug, Clone)]
pub struct Fixture {
    /// `<module>/<fn>`.
    pub name: String,
    pub path: PathBuf,
    pub cases: Vec<Case>,
}

impl Fixture {
    /// Loads `<fixtures root>/<name>.jsonl`.
    pub fn load(name: &str, utf16_fields: Option<&IndexMap<String, String>>) -> Result<Self, String> {
        Self::load_path(
            name,
            &crate::fixtures_root().join(format!("{name}.jsonl")),
            utf16_fields,
        )
    }

    pub fn load_path(name: &str, path: &Path, utf16_fields: Option<&IndexMap<String, String>>) -> Result<Self, String> {
        let text = std::fs::read_to_string(path).map_err(|error| format!("{}: {error}", path.display()))?;
        Self::parse(name, path, &text, utf16_fields)
    }

    pub fn parse(
        name: &str,
        path: &Path,
        text: &str,
        utf16_fields: Option<&IndexMap<String, String>>,
    ) -> Result<Self, String> {
        let cases = text
            .lines()
            .filter(|line| !line.trim().is_empty())
            .enumerate()
            .map(|(index, line)| {
                let raw: Value =
                    serde_json::from_str(line).map_err(|error| format!("{}:{}: {error}", path.display(), index + 1))?;
                Case::from_value(index, raw, utf16_fields)
            })
            .collect::<Result<_, _>>()?;
        Ok(Self {
            name: name.to_string(),
            path: path.to_path_buf(),
            cases,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parses_cases_and_converts_offsets() {
        let text = concat!(
            r#"{"args":["😀a",3],"ret":2}"#,
            "\n",
            r#"{"args":[[1]],"argsAfter":[[1,2]],"ret":{"$u":1}}"#,
            "\n",
            r#"{"args":["😀",1],"throws":"bad"}"#,
            "\n"
        );
        let fields = IndexMap::from([
            ("args.1".to_string(), "args.0".to_string()),
            ("ret".to_string(), "args.0".to_string()),
        ]);
        let fixture = Fixture::parse("m/f", Path::new("m/f.jsonl"), text, Some(&fields)).unwrap();
        assert_eq!(fixture.cases.len(), 3);
        let first = &fixture.cases[0];
        assert_eq!(first.arg_usize(1).unwrap(), 5);
        assert_eq!(first.expect, Expect::Return(json!(4)));
        assert!(first.arg_is_undefined(7));
        assert_eq!(fixture.cases[1].args_after, Some(vec![json!([1, 2])]));
        assert_eq!(fixture.cases[1].expect, Expect::Return(tags::undefined()));
        let third = &fixture.cases[2];
        assert!(
            third
                .conversion_error
                .as_deref()
                .is_some_and(|error| error.contains("surrogate"))
        );
        assert_eq!(third.expect, Expect::Throws("bad".into()));
    }

    #[test]
    fn typed_argument_accessors() {
        let case = Case::from_value(
            0,
            json!({"args": [{"a": 1, "b": {"$u": 1}}, {"$fn": "isEven"}, {"$num": "NaN"}, true], "ret": null}),
            None,
        )
        .unwrap();
        #[derive(serde::Deserialize, Debug, PartialEq)]
        struct Arg {
            a: i64,
            b: Option<i64>,
        }
        assert_eq!(case.arg_as::<Arg>(0).unwrap(), Arg { a: 1, b: None });
        assert_eq!(case.fn_name(1), Some("isEven"));
        assert!(case.arg_f64(2).unwrap().is_nan());
        assert!(case.arg_bool(3).unwrap());
        assert!(case.arg_str(3).unwrap_err().contains("expected a string"));
    }
}
