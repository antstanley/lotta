//! Strict newline-delimited JSON management plane.

use serde::{
    Deserialize, Serialize,
    de::{DeserializeSeed, MapAccess, SeqAccess, Visitor},
};
use serde_json::{Map, Value};
use std::{
    collections::{BTreeMap, BTreeSet, VecDeque},
    io::ErrorKind,
    sync::Arc,
    time::{SystemTime, UNIX_EPOCH},
};
use tokio::io::{AsyncBufReadExt, AsyncWrite, AsyncWriteExt, BufReader};

mod codec;
mod protocol;
mod session;

#[cfg(test)]
mod tests;

pub use codec::{read_line, write_line};
pub use protocol::*;
pub use session::ControlPlane;

#[cfg(test)]
use codec::parse_bounded_json;
