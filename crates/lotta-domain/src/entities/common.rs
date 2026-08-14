use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde_json::{Map, Value};
use std::collections::BTreeMap;
use thiserror::Error;

/// Maximum unknown fields retained for a compatible entity.
pub const EXTRAS_FIELDS_MAX: usize = 128;
/// Maximum elements accepted in string collections, in items.
pub const STRING_ITEMS_MAX: usize = 1_024;
/// Safety maximum for schema-unbounded collections, in items.
pub const UNBOUNDED_COLLECTION_ITEMS_MAX: usize = 4_096;
/// Safety maximum for schema-unbounded maps, in fields.
pub const UNBOUNDED_MAP_FIELDS_MAX: usize = 1_024;
/// Maximum nested JSON depth retained at this boundary, in levels.
pub const JSON_DEPTH_MAX: usize = 64;
/// Maximum elements in any retained nested JSON array, in items.
pub const JSON_ITEMS_MAX: usize = 4_096;
/// Maximum properties in any retained nested JSON object, in fields.
pub const JSON_PROPERTIES_MAX: usize = 1_024;

/// Persistent entity validation failed.
#[derive(Clone, Debug, Eq, Error, PartialEq)]
pub enum EntityError {
    /// A collection exceeded its explicit domain bound.
    #[error("{field} contains {actual} items; maximum is {maximum}")]
    CollectionTooLarge {
        /// Field whose collection was rejected.
        field: &'static str,
        /// Observed item count.
        actual: usize,
        /// Accepted item count.
        maximum: usize,
    },
    /// An unknown field collided with a canonical field.
    #[error("unknown field collides with canonical field: {field}")]
    ExtraFieldCollision {
        /// Colliding field name.
        field: String,
    },
    /// An IANA timezone identifier was invalid.
    #[error("invalid IANA timezone identifier: {value}")]
    InvalidTimezone {
        /// Rejected timezone text.
        value: String,
    },
}

impl EntityError {
    pub(crate) const fn collection(field: &'static str, actual: usize, maximum: usize) -> Self {
        Self::CollectionTooLarge {
            field,
            actual,
            maximum,
        }
    }
}

/// Deterministically ordered, bounded compatible fields.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct EntityExtras(super::BoundedMap<EXTRAS_FIELDS_MAX>);

impl EntityExtras {
    /// Builds extras while rejecting excessive cardinality and known-field collisions.
    ///
    /// # Errors
    /// Returns [`EntityError`] when a bound or collision is violated.
    pub fn new(
        values: BTreeMap<String, Value>,
        known_fields: &[&str],
    ) -> Result<Self, EntityError> {
        for key in values.keys() {
            if known_fields.contains(&key.as_str()) {
                return Err(EntityError::ExtraFieldCollision { field: key.clone() });
            }
        }
        super::BoundedMap::new(values).map(Self)
    }

    /// Returns a retained compatible field.
    #[must_use]
    pub fn get(&self, key: &str) -> Option<&Value> {
        self.0.get(key)
    }

    pub(crate) fn append_to(
        &self,
        map: &mut Map<String, Value>,
        known_fields: &[&str],
    ) -> Result<(), EntityError> {
        for (key, value) in self.0.iter() {
            if known_fields.contains(&key.as_str()) {
                return Err(EntityError::ExtraFieldCollision { field: key.clone() });
            }
            map.insert(key.clone(), value.clone());
        }
        Ok(())
    }
}

impl Serialize for EntityExtras {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        self.0.serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for EntityExtras {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        super::BoundedMap::deserialize(deserializer).map(Self)
    }
}

pub(crate) fn serialize_with_extras<S, T>(
    value: &T,
    extras: &EntityExtras,
    known_fields: &[&str],
    serializer: S,
) -> Result<S::Ok, S::Error>
where
    S: Serializer,
    T: Serialize,
{
    let encoded = serde_json::to_value(value).map_err(serde::ser::Error::custom)?;
    let Value::Object(mut map) = encoded else {
        return Err(serde::ser::Error::custom("entity must serialize as object"));
    };
    extras
        .append_to(&mut map, known_fields)
        .map_err(serde::ser::Error::custom)?;
    map.serialize(serializer)
}
