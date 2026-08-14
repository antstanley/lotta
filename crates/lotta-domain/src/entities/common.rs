use crate::DomainError;
use crate::bounds::EXTRAS_FIELDS_MAX;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde_json::{Map, Value};
use std::collections::BTreeMap;

/// Deterministically ordered, bounded compatible fields.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct EntityExtras(super::BoundedMap<EXTRAS_FIELDS_MAX>);

impl EntityExtras {
    /// Builds extras while rejecting excessive cardinality and known-field collisions.
    ///
    /// # Errors
    /// Returns [`DomainError`] when a bound or collision is violated.
    pub fn new(
        values: BTreeMap<String, Value>,
        known_fields: &[&str],
    ) -> Result<Self, DomainError> {
        for key in values.keys() {
            if known_fields.contains(&key.as_str()) {
                return Err(DomainError::ExtraFieldCollision { field: key.clone() });
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
    ) -> Result<(), DomainError> {
        for (key, value) in self.0.iter() {
            if known_fields.contains(&key.as_str()) {
                return Err(DomainError::ExtraFieldCollision { field: key.clone() });
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
