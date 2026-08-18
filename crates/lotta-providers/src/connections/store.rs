use super::types::{
    AuthFile, ConnectionError, ProviderAuth, ProviderRecord, ProviderSecret, RawFile, RawRecord,
};
use super::{PROVIDER_AUTH_BYTES_MAX, PROVIDER_TEXT_BYTES_MAX};
use lotta_store::atomic::{WriteMode, atomic_write};
use lotta_store::side::{SideRevision, read_opaque, write_opaque_expected};
use lotta_store::{StoreErrorKind, StorePaths};
use serde::de::{self, DeserializeSeed, MapAccess, Visitor};
use serde::ser::{SerializeMap, SerializeStruct};
use serde::{Deserializer, Serialize, Serializer};
use serde_json::{Map, Value};
use std::collections::{BTreeMap, HashSet};
use std::fmt;

/// Baseline-compatible `providers/auth.json` v1 persistence adapter.
pub struct ProviderAuthStore {
    paths: StorePaths,
}

impl ProviderAuthStore {
    /// Creates a store rooted at an already validated local-backend path.
    #[must_use]
    pub const fn new(paths: StorePaths) -> Self {
        Self { paths }
    }

    /// Loads all records and their optimistic persistence revision.
    ///
    /// # Errors
    /// Returns a typed path, shape, bound, conflict, or filesystem failure.
    pub fn load(&self) -> Result<(BTreeMap<String, ProviderRecord>, u64), ConnectionError> {
        let path = self.paths.provider_auth();
        if !path.exists() {
            return Ok((BTreeMap::new(), 0));
        }
        let opaque = read_opaque(&path).map_err(|error| map_store(&error))?;
        let file = parse_file(opaque.bytes())?;
        Ok((file.providers, revision(opaque.revision())))
    }

    /// Atomically writes an exact candidate when the persistence revision remains current.
    ///
    /// # Errors
    /// Returns a typed capacity, conflict, path, lock, or filesystem failure.
    pub fn replace(
        &self,
        records: &BTreeMap<String, ProviderRecord>,
        expected: u64,
    ) -> Result<u64, ConnectionError> {
        crate::limits::validate_provider_count(records.len())
            .map_err(|_| ConnectionError::Capacity)?;
        let path = self.paths.provider_auth();
        let bytes = serialize_file(records)?;
        if expected == 0 && !path.exists() {
            atomic_write(&path, &bytes, WriteMode::ProviderAuth)
                .map_err(|error| map_store(&error))?;
        } else {
            let current = read_opaque(&path).map_err(|error| map_store(&error))?;
            if revision(current.revision()) != expected {
                return Err(ConnectionError::Conflict);
            }
            write_opaque_expected(&path, &bytes, current.revision(), WriteMode::ProviderAuth)
                .map_err(|error| map_store(&error))?;
        }
        read_opaque(&path)
            .map(|value| revision(value.revision()))
            .map_err(|error| map_store(&error))
    }

    /// Re-applies restrictive modes without altering semantic records.
    ///
    /// # Errors
    /// Returns a typed path, conflict, lock, or filesystem failure.
    pub fn enforce_modes(&self) -> Result<(), ConnectionError> {
        let path = self.paths.provider_auth();
        let opaque = read_opaque(&path).map_err(|error| map_store(&error))?;
        write_opaque_expected(
            &path,
            opaque.bytes(),
            opaque.revision(),
            WriteMode::ProviderAuth,
        )
        .map_err(|error| map_store(&error))
    }
}

fn parse_file(bytes: &[u8]) -> Result<AuthFile, ConnectionError> {
    if bytes.len() > PROVIDER_AUTH_BYTES_MAX {
        return Err(ConnectionError::InvalidInput("provider auth bytes"));
    }
    reject_duplicate_keys(bytes)?;
    let mut deserializer = serde_json::Deserializer::from_slice(bytes);
    let raw = UniqueFile.deserialize(&mut deserializer).map_err(|error| {
        if error.to_string().contains("provider limit") {
            ConnectionError::Capacity
        } else {
            ConnectionError::InvalidInput("provider auth JSON")
        }
    })?;
    deserializer
        .end()
        .map_err(|_| ConnectionError::InvalidInput("provider auth JSON"))?;
    validate_extras(&raw.extras)?;
    if raw.version != 1 || crate::limits::validate_provider_count(raw.providers.len()).is_err() {
        return Err(if raw.version == 1 {
            ConnectionError::Capacity
        } else {
            ConnectionError::Unsupported
        });
    }
    let providers = raw
        .providers
        .into_iter()
        .map(validate_record)
        .collect::<Result<_, _>>()?;
    Ok(AuthFile { providers })
}

fn validate_record(
    (key, raw): (String, RawRecord),
) -> Result<(String, ProviderRecord), ConnectionError> {
    for text in [
        &key,
        &raw.id,
        &raw.name,
        &raw.provider_type,
        &raw.created_at,
        &raw.updated_at,
    ] {
        validate_text(text)?;
    }
    if key != raw.name || raw.provider_category != "byok" {
        return Err(ConnectionError::InvalidInput("provider record"));
    }
    validate_optional(raw.access_key.as_ref())?;
    validate_optional(raw.region.as_ref())?;
    validate_optional(raw.profile.as_ref())?;
    validate_optional(raw.base_url.as_ref())?;
    validate_extras(&raw.extras)?;
    let auth = parse_auth(&raw.auth, &raw.provider_type, raw.profile.as_deref())?;
    Ok((
        key,
        ProviderRecord {
            id: raw.id,
            name: raw.name,
            provider_type: raw.provider_type,
            auth,
            access_key: raw.access_key,
            region: raw.region,
            profile: raw.profile,
            base_url: raw.base_url,
            timeout: raw.timeout,
            extras: raw.extras,
            created_at: raw.created_at,
            updated_at: raw.updated_at,
        },
    ))
}

fn parse_auth(
    value: &Value,
    provider_type: &str,
    profile: Option<&str>,
) -> Result<ProviderAuth, ConnectionError> {
    let mut object = value
        .as_object()
        .cloned()
        .ok_or(ConnectionError::InvalidInput("provider auth"))?;
    let method = object
        .remove("type")
        .and_then(|value| value.as_str().map(str::to_owned))
        .ok_or(ConnectionError::InvalidInput("provider auth type"))?;
    match method.as_str() {
        "api" => {
            let key = object
                .remove("key")
                .and_then(|value| value.as_str().map(str::to_owned))
                .ok_or(ConnectionError::InvalidInput("key"))?;
            if key.is_empty()
                && profile.is_some()
                && matches!(provider_type, "bedrock" | "amazon-bedrock")
            {
                validate_extras(&object)?;
                Ok(ProviderAuth::BedrockProfile { extras: object })
            } else {
                let key = ProviderSecret::new(key)?;
                validate_extras(&object)?;
                Ok(ProviderAuth::Api {
                    key,
                    extras: object,
                })
            }
        }
        "oauth" => parse_oauth(object),
        _ => Err(ConnectionError::Unsupported),
    }
}

fn parse_oauth(mut object: Map<String, Value>) -> Result<ProviderAuth, ConnectionError> {
    let access = take_secret(&mut object, "access")?;
    let refresh = take_optional_secret(&mut object, "refresh")?;
    let id_token = take_optional_secret_alias(&mut object, "idToken", "id")?;
    let expires = object
        .remove("expires")
        .and_then(|value| value.as_u64())
        .ok_or(ConnectionError::InvalidInput("OAuth expiry"))?;
    let account_id = take_optional_text(&mut object, "accountId")?;
    for value in object.values() {
        validate_value(value)?;
    }
    Ok(ProviderAuth::OAuth {
        access,
        refresh,
        id_token,
        expires,
        account_id,
        extras: object,
    })
}

fn serialize_file(records: &BTreeMap<String, ProviderRecord>) -> Result<Vec<u8>, ConnectionError> {
    let mut bytes = serde_json::to_vec_pretty(&SerializableFile(records))
        .map_err(|_| ConnectionError::InvalidInput("provider auth serialization"))?;
    bytes.push(b'\n');
    if bytes.len() > PROVIDER_AUTH_BYTES_MAX {
        return Err(ConnectionError::InvalidInput("provider auth bytes"));
    }
    Ok(bytes)
}

struct SerializableFile<'a>(&'a BTreeMap<String, ProviderRecord>);
impl Serialize for SerializableFile<'_> {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut state = serializer.serialize_struct("LocalProviderAuthFile", 2)?;
        state.serialize_field("version", &1)?;
        state.serialize_field("providers", &SerializableRecords(self.0))?;
        state.end()
    }
}
struct SerializableRecords<'a>(&'a BTreeMap<String, ProviderRecord>);
impl Serialize for SerializableRecords<'_> {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut map = serializer.serialize_map(Some(self.0.len()))?;
        for (name, record) in self.0 {
            map.serialize_entry(name, &SerializableRecord(record))?;
        }
        map.end()
    }
}
struct SerializableRecord<'a>(&'a ProviderRecord);
impl Serialize for SerializableRecord<'_> {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let record = self.0;
        let fields = 7
            + usize::from(record.access_key.is_some())
            + usize::from(record.region.is_some())
            + usize::from(record.profile.is_some())
            + usize::from(record.base_url.is_some())
            + usize::from(record.timeout.is_some())
            + record.extras.len();
        let mut state = serializer.serialize_map(Some(fields))?;
        state.serialize_entry("id", &record.id)?;
        state.serialize_entry("name", &record.name)?;
        state.serialize_entry("provider_type", &record.provider_type)?;
        state.serialize_entry("provider_category", "byok")?;
        state.serialize_entry("auth", &SerializableAuth(&record.auth))?;
        if let Some(value) = &record.access_key {
            state.serialize_entry("access_key", value)?;
        }
        if let Some(value) = &record.region {
            state.serialize_entry("region", value)?;
        }
        if let Some(value) = &record.profile {
            state.serialize_entry("profile", value)?;
        }
        if let Some(value) = &record.base_url {
            state.serialize_entry("base_url", value)?;
        }
        if let Some(value) = &record.timeout {
            state.serialize_entry("timeout", value)?;
        }
        for (key, value) in &record.extras {
            state.serialize_entry(key, value)?;
        }
        state.serialize_entry("created_at", &record.created_at)?;
        state.serialize_entry("updated_at", &record.updated_at)?;
        state.end()
    }
}
struct SerializableAuth<'a>(&'a ProviderAuth);
impl Serialize for SerializableAuth<'_> {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        match self.0 {
            ProviderAuth::Api { key, extras } => {
                let mut state = serializer.serialize_map(Some(2 + extras.len()))?;
                state.serialize_entry("type", "api")?;
                state.serialize_entry("key", key.expose())?;
                for (name, value) in extras {
                    state.serialize_entry(name, value)?;
                }
                state.end()
            }
            ProviderAuth::BedrockProfile { extras } => {
                let mut state = serializer.serialize_map(Some(2 + extras.len()))?;
                state.serialize_entry("type", "api")?;
                state.serialize_entry("key", "")?;
                for (name, value) in extras {
                    state.serialize_entry(name, value)?;
                }
                state.end()
            }
            ProviderAuth::OAuth {
                access,
                refresh,
                id_token,
                expires,
                account_id,
                extras,
            } => {
                let fields = 2
                    + usize::from(refresh.is_some())
                    + usize::from(id_token.is_some())
                    + usize::from(account_id.is_some())
                    + extras.len();
                let mut map = serializer.serialize_map(Some(fields))?;
                map.serialize_entry("type", "oauth")?;
                map.serialize_entry("access", access.expose())?;
                if let Some(value) = refresh {
                    map.serialize_entry("refresh", value.expose())?;
                }
                if let Some(value) = id_token {
                    map.serialize_entry("idToken", value.expose())?;
                }
                map.serialize_entry("expires", expires)?;
                if let Some(value) = account_id {
                    map.serialize_entry("accountId", value)?;
                }
                for (key, value) in extras {
                    map.serialize_entry(key, value)?;
                }
                map.end()
            }
        }
    }
}

struct UniqueFile;
impl<'de> DeserializeSeed<'de> for UniqueFile {
    type Value = RawFile;
    fn deserialize<D>(self, deserializer: D) -> Result<Self::Value, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_map(UniqueFileVisitor)
    }
}
struct UniqueFileVisitor;
impl<'de> Visitor<'de> for UniqueFileVisitor {
    type Value = RawFile;
    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("provider auth v1 object")
    }
    fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
    where
        A: MapAccess<'de>,
    {
        let mut version = None;
        let mut providers = None;
        let mut seen = HashSet::new();
        while let Some(key) = map.next_key::<String>()? {
            if !seen.insert(key.clone()) {
                return Err(de::Error::custom("duplicate field"));
            }
            match key.as_str() {
                "version" => version = Some(map.next_value()?),
                "providers" => providers = Some(map.next_value_seed(UniqueProviders)?),
                _ => {
                    let _: Value = map.next_value()?;
                }
            }
        }
        Ok(RawFile {
            version: version.ok_or_else(|| de::Error::missing_field("version"))?,
            providers: providers.ok_or_else(|| de::Error::missing_field("providers"))?,
            extras: Map::new(),
        })
    }
}
struct UniqueProviders;
impl<'de> DeserializeSeed<'de> for UniqueProviders {
    type Value = BTreeMap<String, RawRecord>;
    fn deserialize<D>(self, deserializer: D) -> Result<Self::Value, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_map(UniqueProvidersVisitor)
    }
}
struct UniqueProvidersVisitor;
impl<'de> Visitor<'de> for UniqueProvidersVisitor {
    type Value = BTreeMap<String, RawRecord>;
    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("unique provider record map")
    }
    fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
    where
        A: MapAccess<'de>,
    {
        let mut output = BTreeMap::new();
        while let Some((key, value)) = map.next_entry::<String, RawRecord>()? {
            if output.insert(key, value).is_some() {
                return Err(de::Error::custom("duplicate provider"));
            }
            if crate::limits::validate_provider_count(output.len()).is_err() {
                return Err(de::Error::custom("provider limit"));
            }
        }
        Ok(output)
    }
}

fn reject_duplicate_keys(bytes: &[u8]) -> Result<(), ConnectionError> {
    let text = std::str::from_utf8(bytes)
        .map_err(|_| ConnectionError::InvalidInput("provider auth JSON"))?;
    let mut stack: Vec<HashSet<String>> = Vec::new();
    let mut chars = text.char_indices().peekable();
    while let Some((_, character)) = chars.next() {
        match character {
            '{' => stack.push(HashSet::new()),
            '}' => {
                stack.pop();
            }
            '"' if !stack.is_empty() => {
                let key = scan_json_string(&mut chars)?;
                while chars.peek().is_some_and(|(_, value)| value.is_whitespace()) {
                    chars.next();
                }
                if chars.peek().is_some_and(|(_, value)| *value == ':')
                    && !stack.last_mut().expect("nonempty stack").insert(key)
                {
                    return Err(ConnectionError::InvalidInput("duplicate provider field"));
                }
            }
            _ => {}
        }
    }
    Ok(())
}

fn scan_json_string(
    chars: &mut std::iter::Peekable<std::str::CharIndices<'_>>,
) -> Result<String, ConnectionError> {
    let mut encoded = String::from("\"");
    let mut escaped = false;
    for (_, character) in chars.by_ref() {
        encoded.push(character);
        if escaped {
            escaped = false;
        } else if character == '\\' {
            escaped = true;
        } else if character == '"' {
            return serde_json::from_str(&encoded)
                .map_err(|_| ConnectionError::InvalidInput("provider auth JSON"));
        }
    }
    Err(ConnectionError::InvalidInput("provider auth JSON"))
}

fn validate_text(value: &str) -> Result<(), ConnectionError> {
    if value.is_empty() || value.len() > PROVIDER_TEXT_BYTES_MAX {
        Err(ConnectionError::InvalidInput("provider text"))
    } else {
        Ok(())
    }
}
fn validate_optional(value: Option<&String>) -> Result<(), ConnectionError> {
    if let Some(value) = value {
        validate_text(value)?;
    }
    Ok(())
}
fn validate_extras(extras: &Map<String, Value>) -> Result<(), ConnectionError> {
    if extras.len() > super::PROVIDER_FIELDS_MAX {
        return Err(ConnectionError::Capacity);
    }
    for (key, value) in extras {
        validate_text(key)?;
        validate_value(value)?;
    }
    Ok(())
}

fn validate_value(value: &Value) -> Result<(), ConnectionError> {
    if serde_json::to_vec(value)
        .map_err(|_| ConnectionError::InvalidInput("OAuth extension"))?
        .len()
        > PROVIDER_TEXT_BYTES_MAX
    {
        Err(ConnectionError::InvalidInput("OAuth extension"))
    } else {
        Ok(())
    }
}
fn take_secret(
    object: &mut Map<String, Value>,
    key: &'static str,
) -> Result<ProviderSecret, ConnectionError> {
    object
        .remove(key)
        .and_then(|value| value.as_str().map(str::to_owned))
        .ok_or(ConnectionError::InvalidInput(key))
        .and_then(ProviderSecret::new)
}
fn take_optional_secret(
    object: &mut Map<String, Value>,
    key: &'static str,
) -> Result<Option<ProviderSecret>, ConnectionError> {
    object
        .remove(key)
        .map(|value| {
            value
                .as_str()
                .map(str::to_owned)
                .ok_or(ConnectionError::InvalidInput(key))
                .and_then(ProviderSecret::new)
        })
        .transpose()
}
fn take_optional_secret_alias(
    object: &mut Map<String, Value>,
    first: &'static str,
    second: &'static str,
) -> Result<Option<ProviderSecret>, ConnectionError> {
    let one = take_optional_secret(object, first)?;
    let two = take_optional_secret(object, second)?;
    if one.is_some() && two.is_some() {
        Err(ConnectionError::InvalidInput("duplicate OAuth id token"))
    } else {
        Ok(one.or(two))
    }
}
fn take_optional_text(
    object: &mut Map<String, Value>,
    key: &'static str,
) -> Result<Option<String>, ConnectionError> {
    object
        .remove(key)
        .map(|value| {
            value
                .as_str()
                .map(str::to_owned)
                .ok_or(ConnectionError::InvalidInput(key))
                .and_then(|value| {
                    validate_text(&value)?;
                    Ok(value)
                })
        })
        .transpose()
}
fn revision(value: &SideRevision) -> u64 {
    let text = format!("{value:?}");
    let mut hash = 1_u64;
    for byte in text.bytes() {
        hash = hash
            .wrapping_mul(1_099_511_628_211)
            .wrapping_add(u64::from(byte));
    }
    hash
}
fn map_store(error: &lotta_store::StoreError) -> ConnectionError {
    match error.kind() {
        StoreErrorKind::StorageConflict | StoreErrorKind::LottaLock => ConnectionError::Conflict,
        StoreErrorKind::Limit => ConnectionError::Capacity,
        _ => ConnectionError::Store,
    }
}

#[cfg(test)]
pub(crate) fn parse_for_test(
    bytes: &[u8],
) -> Result<BTreeMap<String, ProviderRecord>, ConnectionError> {
    parse_file(bytes).map(|file| file.providers)
}
#[cfg(test)]
pub(crate) fn serialize_for_test(
    records: &BTreeMap<String, ProviderRecord>,
) -> Result<Vec<u8>, ConnectionError> {
    serialize_file(records)
}
