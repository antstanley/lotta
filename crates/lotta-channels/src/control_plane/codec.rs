use super::{
    AsyncBufReadExt, AsyncWrite, AsyncWriteExt, BTreeSet, BufReader, CONTROL_ARRAY_ITEMS_MAX,
    CONTROL_FRAME_BYTES_MAX, CONTROL_JSON_DEPTH_MAX, CONTROL_MAP_ENTRIES_MAX,
    CONTROL_STRING_BYTES_MAX, ControlError, Deserialize, DeserializeSeed, ErrorKind, Map,
    MapAccess, SeqAccess, Serialize, Value, Visitor,
};
use serde::de::Error as _;

/// Reads and validates one duplicate-free bounded NDJSON management frame.
///
/// # Errors
/// Returns a stable control error for malformed, oversized, duplicate, or I/O input.
pub async fn read_line<R, T>(reader: &mut BufReader<R>) -> Result<Option<T>, ControlError>
where
    R: tokio::io::AsyncRead + Unpin,
    T: for<'de> Deserialize<'de>,
{
    let mut bytes = Vec::new();
    loop {
        let available = reader.fill_buf().await.map_err(|_| ControlError::Io)?;
        if available.is_empty() {
            if bytes.is_empty() {
                return Ok(None);
            }
            return Err(ControlError::Bound);
        }
        let consumed = available
            .iter()
            .position(|byte| *byte == b'\n')
            .map_or(available.len(), |index| index + 1);
        if bytes.len().saturating_add(consumed) > CONTROL_FRAME_BYTES_MAX + 1 {
            return Err(ControlError::Bound);
        }
        bytes.extend_from_slice(&available[..consumed]);
        reader.consume(consumed);
        if bytes.last() == Some(&b'\n') {
            break;
        }
    }
    bytes.pop();
    if bytes.last() == Some(&b'\r') {
        bytes.pop();
    }
    let text = std::str::from_utf8(&bytes).map_err(|_| ControlError::Malformed)?;
    let value = parse_bounded_json(text)?;
    if !value.is_object() {
        return Err(ControlError::Malformed);
    }
    serde_json::from_value(value)
        .map_err(|_| ControlError::Malformed)
        .map(Some)
}

pub(super) fn parse_bounded_json(text: &str) -> Result<Value, ControlError> {
    let mut deserializer = serde_json::Deserializer::from_str(text);
    let value = BoundedValueSeed { depth: 1 }
        .deserialize(&mut deserializer)
        .map_err(|error| map_json_decode(&error))?;
    deserializer.end().map_err(|_| ControlError::Malformed)?;
    Ok(value)
}

fn map_json_decode(error: &serde_json::Error) -> ControlError {
    let text = error.to_string();
    if text.contains("bound") {
        ControlError::Bound
    } else {
        ControlError::Malformed
    }
}

struct BoundedValueSeed {
    depth: usize,
}

impl<'de> DeserializeSeed<'de> for BoundedValueSeed {
    type Value = Value;

    fn deserialize<D>(self, deserializer: D) -> Result<Self::Value, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        if self.depth > CONTROL_JSON_DEPTH_MAX {
            return Err(D::Error::custom("json depth bound"));
        }
        deserializer.deserialize_any(BoundedValueVisitor { depth: self.depth })
    }
}

struct BoundedValueVisitor {
    depth: usize,
}

impl<'de> Visitor<'de> for BoundedValueVisitor {
    type Value = Value;

    fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("bounded JSON value without duplicate keys")
    }

    fn visit_bool<E: serde::de::Error>(self, value: bool) -> Result<Value, E> {
        Ok(Value::Bool(value))
    }

    fn visit_i64<E: serde::de::Error>(self, value: i64) -> Result<Value, E> {
        Ok(Value::Number(value.into()))
    }

    fn visit_u64<E: serde::de::Error>(self, value: u64) -> Result<Value, E> {
        Ok(Value::Number(value.into()))
    }

    fn visit_f64<E: serde::de::Error>(self, value: f64) -> Result<Value, E> {
        serde_json::Number::from_f64(value)
            .map(Value::Number)
            .ok_or_else(|| E::custom("non-finite number"))
    }

    fn visit_none<E: serde::de::Error>(self) -> Result<Value, E> {
        Ok(Value::Null)
    }

    fn visit_unit<E: serde::de::Error>(self) -> Result<Value, E> {
        Ok(Value::Null)
    }

    fn visit_str<E: serde::de::Error>(self, value: &str) -> Result<Value, E> {
        self.visit_string(value.to_owned())
    }

    fn visit_string<E: serde::de::Error>(self, value: String) -> Result<Value, E> {
        if value.len() > CONTROL_STRING_BYTES_MAX {
            return Err(E::custom("json string bound"));
        }
        Ok(Value::String(value))
    }

    fn visit_seq<A>(self, mut sequence: A) -> Result<Value, A::Error>
    where
        A: SeqAccess<'de>,
    {
        let mut values = Vec::new();
        while let Some(value) = sequence.next_element_seed(BoundedValueSeed {
            depth: self.depth + 1,
        })? {
            if values.len() >= CONTROL_ARRAY_ITEMS_MAX {
                return Err(A::Error::custom("json array bound"));
            }
            values.push(value);
        }
        Ok(Value::Array(values))
    }

    fn visit_map<A>(self, mut object: A) -> Result<Value, A::Error>
    where
        A: MapAccess<'de>,
    {
        let mut values = Map::new();
        let mut keys = BTreeSet::new();
        while let Some(key) = object.next_key::<String>()? {
            if key.len() > CONTROL_STRING_BYTES_MAX
                || values.len() >= CONTROL_MAP_ENTRIES_MAX
                || !keys.insert(key.clone())
            {
                return Err(A::Error::custom("json object bound or duplicate key"));
            }
            let value = object.next_value_seed(BoundedValueSeed {
                depth: self.depth + 1,
            })?;
            values.insert(key, value);
        }
        Ok(Value::Object(values))
    }
}

/// Writes exactly one bounded JSON object followed by one newline.
///
/// # Errors
/// Returns an encoding, byte-bound, or pipe failure.
pub async fn write_line<W, T>(writer: &mut W, frame: &T) -> Result<(), ControlError>
where
    W: AsyncWrite + Unpin,
    T: Serialize + ?Sized,
{
    let bytes = serde_json::to_vec(frame).map_err(|_| ControlError::Malformed)?;
    if bytes.is_empty() || bytes.len() > CONTROL_FRAME_BYTES_MAX {
        return Err(ControlError::Bound);
    }
    writer.write_all(&bytes).await.map_err(map_io)?;
    writer.write_all(b"\n").await.map_err(map_io)?;
    writer.flush().await.map_err(map_io)
}

fn map_io(error: std::io::Error) -> ControlError {
    let _closed = matches!(
        error.kind(),
        ErrorKind::BrokenPipe | ErrorKind::UnexpectedEof
    );
    drop(error);
    ControlError::Io
}
