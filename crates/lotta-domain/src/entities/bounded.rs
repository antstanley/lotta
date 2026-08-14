//! Bounded collection primitives for persistent entities.

use super::{JSON_DEPTH_MAX, JSON_ITEMS_MAX, JSON_PROPERTIES_MAX};
use crate::DomainError;
use serde::{
    Deserialize, Deserializer, Serialize, Serializer,
    de::{self, DeserializeSeed, MapAccess, SeqAccess, Visitor},
};
use serde_json::{Map, Number, Value};
use std::{collections::BTreeMap, fmt, marker::PhantomData};

/// A vector whose length cannot exceed `MAX` through safe public APIs.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct BoundedVec<T, const MAX: usize>(Vec<T>);

impl<T, const MAX: usize> BoundedVec<T, MAX> {
    /// Creates a bounded vector.
    ///
    /// # Errors
    /// Returns [`DomainError`] if the vector exceeds `MAX` elements.
    pub fn new(values: Vec<T>) -> Result<Self, DomainError> {
        if values.len() > MAX {
            return Err(DomainError::collection("list", values.len(), MAX));
        }
        Ok(Self(values))
    }

    /// Returns the number of elements.
    #[must_use]
    pub fn len(&self) -> usize {
        self.0.len()
    }

    /// Returns whether the vector is empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// Borrows the elements.
    #[must_use]
    pub fn as_slice(&self) -> &[T] {
        &self.0
    }
}

impl<T: Serialize, const MAX: usize> Serialize for BoundedVec<T, MAX> {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        self.0.serialize(serializer)
    }
}

struct VecVisitor<T, const MAX: usize>(PhantomData<T>);

impl<'de, T: Deserialize<'de>, const MAX: usize> Visitor<'de> for VecVisitor<T, MAX> {
    type Value = BoundedVec<T, MAX>;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "at most {MAX} list items")
    }

    fn visit_seq<A: SeqAccess<'de>>(self, mut sequence: A) -> Result<Self::Value, A::Error> {
        let capacity = sequence.size_hint().unwrap_or(0).min(MAX);
        let mut values = Vec::with_capacity(capacity);
        while let Some(value) = sequence.next_element()? {
            if values.len() == MAX {
                return Err(de::Error::custom(DomainError::collection(
                    "list",
                    MAX + 1,
                    MAX,
                )));
            }
            values.push(value);
        }
        Ok(BoundedVec(values))
    }
}

impl<'de, T: Deserialize<'de>, const MAX: usize> Deserialize<'de> for BoundedVec<T, MAX> {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        deserializer.deserialize_seq(VecVisitor::<T, MAX>(PhantomData))
    }
}

/// A JSON value bounded recursively during construction and deserialization.
#[derive(Clone, Debug, PartialEq)]
pub struct BoundedJsonValue(Value);

impl BoundedJsonValue {
    /// Validates and wraps an existing JSON value.
    ///
    /// # Errors
    /// Returns [`DomainError`] when a nested JSON limit is exceeded.
    pub fn new(value: Value) -> Result<Self, DomainError> {
        validate_json(&value, 0)?;
        Ok(Self(value))
    }

    /// Borrows the canonical JSON value.
    #[must_use]
    pub const fn as_value(&self) -> &Value {
        &self.0
    }

    fn into_value(self) -> Value {
        self.0
    }
}

impl Serialize for BoundedJsonValue {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        self.0.serialize(serializer)
    }
}

#[derive(Clone, Copy)]
struct JsonSeed {
    depth: usize,
}

impl JsonSeed {
    fn child<E: de::Error>(self) -> Result<Self, E> {
        let depth = self.depth + 1;
        if depth > JSON_DEPTH_MAX {
            return Err(E::custom(DomainError::collection(
                "JSON depth",
                depth,
                JSON_DEPTH_MAX,
            )));
        }
        Ok(Self { depth })
    }
}

struct JsonVisitor {
    depth: usize,
}

impl<'de> DeserializeSeed<'de> for JsonSeed {
    type Value = BoundedJsonValue;

    fn deserialize<D: Deserializer<'de>>(self, deserializer: D) -> Result<Self::Value, D::Error> {
        deserializer.deserialize_any(JsonVisitor { depth: self.depth })
    }
}

impl<'de> Visitor<'de> for JsonVisitor {
    type Value = BoundedJsonValue;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a bounded JSON value")
    }

    fn visit_bool<E: de::Error>(self, value: bool) -> Result<Self::Value, E> {
        Ok(BoundedJsonValue(Value::Bool(value)))
    }

    fn visit_i64<E: de::Error>(self, value: i64) -> Result<Self::Value, E> {
        Ok(BoundedJsonValue(Value::Number(value.into())))
    }

    fn visit_u64<E: de::Error>(self, value: u64) -> Result<Self::Value, E> {
        Ok(BoundedJsonValue(Value::Number(value.into())))
    }

    fn visit_f64<E: de::Error>(self, value: f64) -> Result<Self::Value, E> {
        Number::from_f64(value)
            .map(Value::Number)
            .map(BoundedJsonValue)
            .ok_or_else(|| E::custom("non-finite JSON number"))
    }

    fn visit_str<E: de::Error>(self, value: &str) -> Result<Self::Value, E> {
        Ok(BoundedJsonValue(Value::String(value.to_owned())))
    }

    fn visit_string<E: de::Error>(self, value: String) -> Result<Self::Value, E> {
        Ok(BoundedJsonValue(Value::String(value)))
    }

    fn visit_none<E: de::Error>(self) -> Result<Self::Value, E> {
        Ok(BoundedJsonValue(Value::Null))
    }

    fn visit_unit<E: de::Error>(self) -> Result<Self::Value, E> {
        Ok(BoundedJsonValue(Value::Null))
    }

    fn visit_some<D: Deserializer<'de>>(self, deserializer: D) -> Result<Self::Value, D::Error> {
        JsonSeed { depth: self.depth }.deserialize(deserializer)
    }

    fn visit_seq<A: SeqAccess<'de>>(self, mut sequence: A) -> Result<Self::Value, A::Error> {
        let capacity = sequence.size_hint().unwrap_or(0).min(JSON_ITEMS_MAX);
        let mut values = Vec::with_capacity(capacity);
        let seed = JsonSeed { depth: self.depth }.child()?;
        while let Some(value) = sequence.next_element_seed(seed)? {
            if values.len() == JSON_ITEMS_MAX {
                return Err(de::Error::custom(DomainError::collection(
                    "JSON array",
                    JSON_ITEMS_MAX + 1,
                    JSON_ITEMS_MAX,
                )));
            }
            values.push(value.into_value());
        }
        Ok(BoundedJsonValue(Value::Array(values)))
    }

    fn visit_map<A: MapAccess<'de>>(self, mut access: A) -> Result<Self::Value, A::Error> {
        let capacity = access.size_hint().unwrap_or(0).min(JSON_PROPERTIES_MAX);
        let mut values = Map::with_capacity(capacity);
        let seed = JsonSeed { depth: self.depth }.child()?;
        while let Some(key) = access.next_key::<String>()? {
            if values.len() == JSON_PROPERTIES_MAX {
                return Err(de::Error::custom(DomainError::collection(
                    "JSON object",
                    JSON_PROPERTIES_MAX + 1,
                    JSON_PROPERTIES_MAX,
                )));
            }
            values.insert(key, access.next_value_seed(seed)?.into_value());
        }
        Ok(BoundedJsonValue(Value::Object(values)))
    }
}

impl<'de> Deserialize<'de> for BoundedJsonValue {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        JsonSeed { depth: 0 }.deserialize(deserializer)
    }
}

/// A map whose property count cannot exceed `MAX` through safe public APIs.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct BoundedMap<const MAX: usize>(BTreeMap<String, BoundedJsonValue>);

impl<const MAX: usize> BoundedMap<MAX> {
    /// Creates a bounded map and validates all retained JSON recursively.
    ///
    /// # Errors
    /// Returns [`DomainError`] if direct or nested JSON bounds are exceeded.
    pub fn new(values: BTreeMap<String, Value>) -> Result<Self, DomainError> {
        if values.len() > MAX {
            return Err(DomainError::collection("object", values.len(), MAX));
        }
        values
            .into_iter()
            .map(|(key, value)| BoundedJsonValue::new(value).map(|value| (key, value)))
            .collect::<Result<_, _>>()
            .map(Self)
    }

    /// Returns the property count.
    #[must_use]
    pub fn len(&self) -> usize {
        self.0.len()
    }

    /// Returns whether the map is empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// Borrows a property.
    #[must_use]
    pub fn get(&self, key: &str) -> Option<&Value> {
        self.0.get(key).map(BoundedJsonValue::as_value)
    }

    pub(crate) fn iter(&self) -> impl Iterator<Item = (&String, &Value)> {
        self.0.iter().map(|(key, value)| (key, value.as_value()))
    }
}

impl<const MAX: usize> Serialize for BoundedMap<MAX> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        self.0.serialize(serializer)
    }
}

struct MapVisitor<const MAX: usize>;

impl<'de, const MAX: usize> Visitor<'de> for MapVisitor<MAX> {
    type Value = BoundedMap<MAX>;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "at most {MAX} object properties")
    }

    fn visit_map<A: MapAccess<'de>>(self, mut access: A) -> Result<Self::Value, A::Error> {
        let mut values = BTreeMap::new();
        while let Some(key) = access.next_key::<String>()? {
            if values.len() == MAX {
                return Err(de::Error::custom(DomainError::collection(
                    "object",
                    MAX + 1,
                    MAX,
                )));
            }
            let value = access.next_value_seed(JsonSeed { depth: 0 })?;
            values.insert(key, value);
        }
        Ok(BoundedMap(values))
    }
}

impl<'de, const MAX: usize> Deserialize<'de> for BoundedMap<MAX> {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        deserializer.deserialize_map(MapVisitor::<MAX>)
    }
}

fn validate_json(value: &Value, depth: usize) -> Result<(), DomainError> {
    if depth > JSON_DEPTH_MAX {
        return Err(DomainError::collection("JSON depth", depth, JSON_DEPTH_MAX));
    }
    match value {
        Value::Array(values) => validate_array(values, depth),
        Value::Object(values) => validate_object(values, depth),
        _ => Ok(()),
    }
}

fn validate_array(values: &[Value], depth: usize) -> Result<(), DomainError> {
    if values.len() > JSON_ITEMS_MAX {
        return Err(DomainError::collection(
            "JSON array",
            values.len(),
            JSON_ITEMS_MAX,
        ));
    }
    values
        .iter()
        .try_for_each(|value| validate_json(value, depth + 1))
}

fn validate_object(values: &Map<String, Value>, depth: usize) -> Result<(), DomainError> {
    if values.len() > JSON_PROPERTIES_MAX {
        return Err(DomainError::collection(
            "JSON object",
            values.len(),
            JSON_PROPERTIES_MAX,
        ));
    }
    values
        .values()
        .try_for_each(|value| validate_json(value, depth + 1))
}
