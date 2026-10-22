//! JSON object key order: keeps the key order of the existing wire and on-disk formats.
//!
//! `JSON.stringify` writes keys in insertion order, and the TS daemon builds most records by spreading
//! (`{...session, doneAt}`), so a record's key order depends on its history (and, for state.json and config.json, on
//! the file it was read from), not on its interface. A plain struct always serialises in declaration order; where TS
//! output order is history-dependent the protocol wraps the struct in [`Ordered`], which remembers the key order the
//! value was read with (recursively) and writes it back that way. Keys the Rust side adds later follow, in struct
//! order, unless [`Ordered::touch`] records them in JS insertion order.

use std::collections::HashMap;
use std::fmt;
use std::ops::{Deref, DerefMut};

use serde::de::{DeserializeOwned, Error as _};
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde_json::{Map, Value};

/// The key order of a JSON value: per object its keys (with their children's order), per array its items.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum KeyOrder {
    /// No recorded order (a scalar, or a value built on the Rust side): struct order applies.
    #[default]
    Unrecorded,
    Object(Vec<(String, KeyOrder)>),
    Array(Vec<KeyOrder>),
}

impl KeyOrder {
    /// The key order of `value`, recursively.
    pub fn of(value: &Value) -> Self {
        match value {
            Value::Object(map) => KeyOrder::Object(
                map.iter()
                    .map(|(key, child)| (key.clone(), KeyOrder::of(child)))
                    .collect(),
            ),
            Value::Array(items) if items.iter().any(|item| item.is_object() || item.is_array()) => {
                KeyOrder::Array(items.iter().map(KeyOrder::of).collect())
            }
            _ => KeyOrder::Unrecorded,
        }
    }

    /// Reorders `value` by this order: recorded keys first (in their recorded order), then any others in the order
    /// they already have. Array items are matched by index.
    pub fn apply(&self, value: Value) -> Value {
        match (self, value) {
            (KeyOrder::Object(keys), Value::Object(map)) => {
                let mut slots: Vec<Option<(String, Value)>> = map.into_iter().map(Some).collect();
                let index: HashMap<String, usize> = slots
                    .iter()
                    .enumerate()
                    .filter_map(|(i, slot)| slot.as_ref().map(|(key, _)| (key.clone(), i)))
                    .collect();
                let mut out = Map::new();
                for (key, child) in keys {
                    if let Some(slot) = index.get(key).and_then(|&i| slots.get_mut(i)).and_then(Option::take) {
                        out.insert(slot.0, child.apply(slot.1));
                    }
                }
                for (key, child) in slots.into_iter().flatten() {
                    out.insert(key, child);
                }
                Value::Object(out)
            }
            (KeyOrder::Array(orders), Value::Array(items)) => Value::Array(
                items
                    .into_iter()
                    .enumerate()
                    .map(|(i, item)| match orders.get(i) {
                        Some(order) => order.apply(item),
                        None => item,
                    })
                    .collect(),
            ),
            (_, value) => value,
        }
    }
}

/// A value that serialises with the key order it was deserialised with (see the module docs). Derefs to the value.
#[derive(Clone, Default)]
pub struct Ordered<T> {
    value: T,
    order: KeyOrder,
}

impl<T> Ordered<T> {
    /// A value built on the Rust side: serialised in struct order.
    pub fn new(value: T) -> Self {
        Ordered {
            value,
            order: KeyOrder::Unrecorded,
        }
    }

    /// A value with an explicit key order (e.g. reproducing a TS spread like `{sessionId, ...record}`).
    pub fn with_order(value: T, order: KeyOrder) -> Self {
        Ordered { value, order }
    }

    pub fn into_inner(self) -> T {
        self.value
    }

    pub fn key_order(&self) -> &KeyOrder {
        &self.order
    }

    pub fn set_key_order(&mut self, order: KeyOrder) {
        self.order = order;
    }
}

impl<T: Serialize> Ordered<T> {
    /// Records `key` as set now: like assigning a key a JS object does not have yet, it moves to the end (a key it
    /// already has keeps its place, as in JS).
    pub fn touch(&mut self, key: &str) {
        if self.order == KeyOrder::Unrecorded
            && let Ok(value) = serde_json::to_value(&self.value)
        {
            self.order = KeyOrder::of(&value);
        }
        match &mut self.order {
            KeyOrder::Object(keys) => {
                if !keys.iter().any(|(existing, _)| existing == key) {
                    keys.push((key.to_owned(), KeyOrder::Unrecorded));
                }
            }
            order => *order = KeyOrder::Object(vec![(key.to_owned(), KeyOrder::Unrecorded)]),
        }
    }

    /// The value as JSON, in its recorded key order.
    pub fn to_value(&self) -> serde_json::Result<Value> {
        Ok(self.order.apply(serde_json::to_value(&self.value)?))
    }
}

impl<T> From<T> for Ordered<T> {
    fn from(value: T) -> Self {
        Ordered::new(value)
    }
}

impl<T> Deref for Ordered<T> {
    type Target = T;
    fn deref(&self) -> &T {
        &self.value
    }
}

impl<T> DerefMut for Ordered<T> {
    fn deref_mut(&mut self) -> &mut T {
        &mut self.value
    }
}

impl<T: PartialEq> PartialEq for Ordered<T> {
    fn eq(&self, other: &Self) -> bool {
        self.value == other.value
    }
}

impl<T: fmt::Debug> fmt::Debug for Ordered<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.value.fmt(f)
    }
}

impl<T: Serialize> Serialize for Ordered<T> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        if self.order == KeyOrder::Unrecorded {
            return self.value.serialize(serializer);
        }
        self.to_value()
            .map_err(serde::ser::Error::custom)?
            .serialize(serializer)
    }
}

impl<'de, T: DeserializeOwned> Deserialize<'de> for Ordered<T> {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let value = Value::deserialize(deserializer)?;
        let order = KeyOrder::of(&value);
        let value = T::deserialize(value).map_err(D::Error::custom)?;
        Ok(Ordered { value, order })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Serialize, Deserialize, Debug, PartialEq)]
    struct Pair {
        a: u32,
        #[serde(skip_serializing_if = "Option::is_none")]
        b: Option<u32>,
        #[serde(skip_serializing_if = "Option::is_none")]
        c: Option<u32>,
    }

    #[test]
    fn keeps_the_order_it_was_read_with() {
        let ordered: Ordered<Pair> = serde_json::from_str(r#"{"b":2,"a":1}"#).unwrap();
        assert_eq!(serde_json::to_string(&ordered).unwrap(), r#"{"b":2,"a":1}"#);
        assert_eq!(
            serde_json::to_string(&Ordered::new(Pair {
                a: 1,
                b: Some(2),
                c: None
            }))
            .unwrap(),
            r#"{"a":1,"b":2}"#
        );
    }

    #[test]
    fn touch_appends_new_keys_like_js_assignment() {
        let mut ordered: Ordered<Pair> = serde_json::from_str(r#"{"b":2,"a":1}"#).unwrap();
        ordered.c = Some(3);
        assert_eq!(serde_json::to_string(&ordered).unwrap(), r#"{"b":2,"a":1,"c":3}"#);
        let mut fresh = Ordered::new(Pair {
            a: 1,
            b: None,
            c: Some(3),
        });
        fresh.touch("b");
        fresh.b = Some(2);
        assert_eq!(serde_json::to_string(&fresh).unwrap(), r#"{"a":1,"c":3,"b":2}"#);
    }
}
