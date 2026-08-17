//! Validated public MCP identifiers and server configuration.

use serde::{Deserialize, Deserializer, Serialize};
use std::{collections::BTreeMap, fmt, path::Path};
use url::Url;

const IDENTIFIER_BYTES_MAX: usize = 256;
const COMMAND_BYTES_MAX: usize = 4_096;
const ARGUMENTS_MAX: usize = 256;
const ARGUMENT_BYTES_MAX: usize = 16 * 1024;
const ENVIRONMENT_ITEMS_MAX: usize = 256;
const URL_BYTES_MAX: usize = 8 * 1024;

macro_rules! bounded_id {
    ($name:ident, $label:literal) => {
        #[doc = concat!("Validated ", $label, ".")]
        #[derive(Clone, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
        #[serde(transparent)]
        pub struct $name(String);
        impl $name {
            #[doc = concat!("Validates a non-empty bounded ", $label, ".")]
            pub fn new(value: String) -> Result<Self, McpConfigError> {
                if value.is_empty() || value.len() > IDENTIFIER_BYTES_MAX || value.contains('\0') {
                    return Err(McpConfigError::Identifier);
                }
                Ok(Self(value))
            }
            /// Borrows the exact identifier.
            #[must_use]
            pub fn as_str(&self) -> &str {
                &self.0
            }
        }
        impl fmt::Debug for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(concat!(stringify!($name), "([redacted])"))
            }
        }
        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("[redacted]")
            }
        }
        impl<'de> Deserialize<'de> for $name {
            fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
                Self::new(String::deserialize(d)?).map_err(serde::de::Error::custom)
            }
        }
    };
}

bounded_id!(ServerId, "MCP server identifier");
bounded_id!(CredentialRef, "credential-store reference");
bounded_id!(SessionId, "MCP session identifier");
bounded_id!(RequestId, "MCP request identifier");

/// Fixed secret-free configuration failure.
#[derive(Clone, Copy, Debug, Eq, PartialEq, thiserror::Error)]
pub enum McpConfigError {
    /// Identifier was empty, oversized, or contained NUL.
    #[error("invalid MCP identifier")]
    Identifier,
    /// Command or argument contract was unsafe or over bound.
    #[error("invalid MCP stdio command")]
    Command,
    /// URL was relative, unsupported, credential-bearing, or over bound.
    #[error("invalid MCP URL")]
    Url,
    /// Header/environment collection was unsafe or over bound.
    #[error("invalid MCP configuration fields")]
    Fields,
}

/// Validated stdio server configuration. Omitted `type` selects this variant.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct StdioConfig {
    /// Stable server identifier.
    pub name: ServerId,
    /// Absolute executable path, launched directly without a shell.
    pub command: String,
    /// Exact argument vector.
    #[serde(default)]
    pub args: Vec<String>,
    /// Explicit environment values; no ambient environment is inherited.
    #[serde(default)]
    pub env: BTreeMap<String, String>,
    /// Optional absolute working directory.
    pub cwd: Option<String>,
    /// Optional credential-store reference delivered to the child environment.
    pub credential: Option<CredentialRef>,
}

/// Validated legacy SSE server configuration.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SseConfig {
    /// Stable server identifier.
    pub name: ServerId,
    /// Initial SSE endpoint.
    pub url: String,
    /// Optional credential-store reference for Authorization.
    pub credential: Option<CredentialRef>,
}

/// Validated streamable HTTP server configuration.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct HttpConfig {
    /// Stable server identifier.
    pub name: ServerId,
    /// Streamable HTTP endpoint.
    pub url: String,
    /// Optional credential-store reference for Authorization.
    pub credential: Option<CredentialRef>,
}

/// Exact MCP transport discriminants.
#[derive(Clone, Debug, Serialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum McpServerConfig {
    /// Local sidecar over Task 42 framing.
    Stdio(StdioConfig),
    /// Legacy HTTP GET event stream plus POST endpoint.
    Sse(SseConfig),
    /// Streamable HTTP POST transport.
    Http(HttpConfig),
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ConfigWire {
    #[serde(rename = "type")]
    kind: Option<String>,
    name: ServerId,
    command: Option<String>,
    #[serde(default)]
    args: Vec<String>,
    #[serde(default)]
    env: BTreeMap<String, String>,
    cwd: Option<String>,
    url: Option<String>,
    credential: Option<CredentialRef>,
}

impl<'de> Deserialize<'de> for McpServerConfig {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let wire = ConfigWire::deserialize(deserializer)?;
        from_wire(wire).map_err(serde::de::Error::custom)
    }
}

fn from_wire(wire: ConfigWire) -> Result<McpServerConfig, McpConfigError> {
    match wire.kind.as_deref().unwrap_or("stdio") {
        "stdio" => {
            if wire.url.is_some() {
                return Err(McpConfigError::Fields);
            }
            let config = StdioConfig {
                name: wire.name,
                command: wire.command.ok_or(McpConfigError::Command)?,
                args: wire.args,
                env: wire.env,
                cwd: wire.cwd,
                credential: wire.credential,
            };
            validate_stdio(&config)?;
            Ok(McpServerConfig::Stdio(config))
        }
        "sse" | "http" => {
            if wire.command.is_some()
                || !wire.args.is_empty()
                || !wire.env.is_empty()
                || wire.cwd.is_some()
            {
                return Err(McpConfigError::Fields);
            }
            let url = wire.url.ok_or(McpConfigError::Url)?;
            validate_url(&url)?;
            if wire.kind.as_deref() == Some("sse") {
                Ok(McpServerConfig::Sse(SseConfig {
                    name: wire.name,
                    url,
                    credential: wire.credential,
                }))
            } else {
                Ok(McpServerConfig::Http(HttpConfig {
                    name: wire.name,
                    url,
                    credential: wire.credential,
                }))
            }
        }
        _ => Err(McpConfigError::Fields),
    }
}

fn validate_stdio(config: &StdioConfig) -> Result<(), McpConfigError> {
    let command = Path::new(&config.command);
    if config.command.len() > COMMAND_BYTES_MAX
        || !command.is_absolute()
        || config.command.contains('\0')
    {
        return Err(McpConfigError::Command);
    }
    if config.args.len() > ARGUMENTS_MAX || config.env.len() > ENVIRONMENT_ITEMS_MAX {
        return Err(McpConfigError::Fields);
    }
    if config
        .args
        .iter()
        .any(|arg| arg.len() > ARGUMENT_BYTES_MAX || arg.contains('\0'))
    {
        return Err(McpConfigError::Command);
    }
    if config.env.iter().any(|(key, value)| !valid_env(key, value)) {
        return Err(McpConfigError::Fields);
    }
    if config
        .cwd
        .as_ref()
        .is_some_and(|cwd| !Path::new(cwd).is_absolute())
    {
        return Err(McpConfigError::Command);
    }
    Ok(())
}

fn valid_env(key: &str, value: &str) -> bool {
    !key.is_empty()
        && key.len() <= IDENTIFIER_BYTES_MAX
        && value.len() <= ARGUMENT_BYTES_MAX
        && !key.contains(['=', '\0'])
        && !value.contains('\0')
}

/// Validates an absolute HTTP(S) URL without user information, fragment, or query credentials.
pub(crate) fn validate_url(value: &str) -> Result<Url, McpConfigError> {
    if value.len() > URL_BYTES_MAX {
        return Err(McpConfigError::Url);
    }
    let url = Url::parse(value).map_err(|_| McpConfigError::Url)?;
    if !matches!(url.scheme(), "http" | "https")
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.fragment().is_some()
        || url.query().is_some()
    {
        return Err(McpConfigError::Url);
    }
    Ok(url)
}
