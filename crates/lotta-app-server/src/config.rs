use std::{net::IpAddr, path::PathBuf};

use url::Url;

use crate::{auth::AuthPolicy, error::AppServerError};

/// Largest accepted signed-token clock skew.
pub const AUTH_CLOCK_SKEW_SECONDS_MAX: u32 = 300;
/// Baseline signed-token clock skew.
pub const AUTH_CLOCK_SKEW_SECONDS_DEFAULT: u32 = 30;

/// Raw command-line values, validated before any listener bind.
#[derive(Clone, Debug, Default)]
pub struct ServerArgs {
    /// Listen URL; `None` after bare `--listen` means the loopback ephemeral default.
    pub listen: Option<String>,
    /// Whether `--listen` was supplied.
    pub listen_enabled: bool,
    /// Whether OpenAI-compatible routes are advertised.
    pub openai_api: bool,
    /// Authentication mode text.
    pub ws_auth: Option<String>,
    /// Capability token file.
    pub ws_token_file: Option<PathBuf>,
    /// Capability token SHA-256 hex digest.
    pub ws_token_sha256: Option<String>,
    /// Signed bearer shared-secret file.
    pub ws_shared_secret_file: Option<PathBuf>,
    /// Required issuer when set.
    pub ws_issuer: Option<String>,
    /// Required audience when set.
    pub ws_audience: Option<String>,
    /// Signed bearer clock skew.
    pub ws_max_clock_skew_seconds: Option<u32>,
}

/// Validated listener configuration that contains no plaintext token file value.
#[derive(Debug)]
pub struct PreparedServer {
    /// Bind host.
    pub(crate) host: String,
    /// Bind port.
    pub(crate) port: u16,
    /// Exact configured websocket path.
    pub(crate) websocket_path: String,
    /// Whether to advertise the `OpenAI` base URL.
    pub(crate) openai_api: bool,
    /// Prepared authentication policy.
    pub(crate) auth: AuthPolicy,
}

/// Parses the exact Task-14 command-line surface.
///
/// # Errors
/// Returns a stable configuration error for unknown, duplicate, missing, or malformed arguments.
pub fn parse_cli<I, S>(arguments: I) -> Result<ServerArgs, AppServerError>
where
    I: IntoIterator<Item = S>,
    S: Into<String>,
{
    let values: Vec<String> = arguments.into_iter().map(Into::into).collect();
    let mut args = ServerArgs::default();
    let mut index = parse_prefix(&values)?;
    while index < values.len() {
        index = parse_flag(&values, index, &mut args)?;
    }
    if !args.listen_enabled {
        return Err(AppServerError::Config("--listen is required"));
    }
    Ok(args)
}

fn parse_prefix(values: &[String]) -> Result<usize, AppServerError> {
    let mut index = usize::from(values.first().is_some_and(|value| value == "server"));
    if values.get(index).is_some_and(|value| value == "--backend") {
        if values.get(index + 1).map(String::as_str) != Some("local") {
            return Err(AppServerError::Config("--backend must be local"));
        }
        index += 2;
    }
    Ok(index)
}

fn parse_flag(
    values: &[String],
    index: usize,
    args: &mut ServerArgs,
) -> Result<usize, AppServerError> {
    match values[index].as_str() {
        "--listen" => parse_listen(values, index, args),
        "--openai-api" => {
            if args.openai_api {
                return Err(AppServerError::Config("--openai-api may appear only once"));
            }
            args.openai_api = true;
            Ok(index + 1)
        }
        "--ws-auth" => set_string(values, index, &mut args.ws_auth),
        "--ws-token-file" => set_path(values, index, &mut args.ws_token_file),
        "--ws-token-sha256" => set_string(values, index, &mut args.ws_token_sha256),
        "--ws-shared-secret-file" => set_path(values, index, &mut args.ws_shared_secret_file),
        "--ws-issuer" => set_string(values, index, &mut args.ws_issuer),
        "--ws-audience" => set_string(values, index, &mut args.ws_audience),
        "--ws-max-clock-skew-seconds" => set_skew(values, index, args),
        _ => Err(AppServerError::Config("unknown server argument")),
    }
}

fn parse_listen(
    values: &[String],
    index: usize,
    args: &mut ServerArgs,
) -> Result<usize, AppServerError> {
    if args.listen_enabled {
        return Err(AppServerError::Config("--listen may appear only once"));
    }
    args.listen_enabled = true;
    match values.get(index + 1) {
        Some(value) if !value.starts_with('-') => {
            args.listen = Some(value.clone());
            Ok(index + 2)
        }
        _ => Ok(index + 1),
    }
}

fn set_string(
    values: &[String],
    index: usize,
    target: &mut Option<String>,
) -> Result<usize, AppServerError> {
    if target.is_some() {
        return Err(AppServerError::Config("argument may appear only once"));
    }
    let value = values
        .get(index + 1)
        .filter(|value| !value.starts_with('-'))
        .ok_or(AppServerError::Config("argument requires a value"))?;
    *target = Some(value.clone());
    Ok(index + 2)
}

fn set_path(
    values: &[String],
    index: usize,
    target: &mut Option<PathBuf>,
) -> Result<usize, AppServerError> {
    let mut value = None;
    let next = set_string(values, index, &mut value)?;
    *target = value.map(PathBuf::from);
    Ok(next)
}

fn set_skew(
    values: &[String],
    index: usize,
    args: &mut ServerArgs,
) -> Result<usize, AppServerError> {
    if args.ws_max_clock_skew_seconds.is_some() {
        return Err(AppServerError::Config("argument may appear only once"));
    }
    let value = values
        .get(index + 1)
        .ok_or(AppServerError::Config("argument requires a value"))?;
    args.ws_max_clock_skew_seconds = Some(
        value
            .parse()
            .map_err(|_| AppServerError::Config("clock skew must be a u32"))?,
    );
    Ok(index + 2)
}

impl ServerArgs {
    /// Validates URL/auth configuration and reads secrets once, before bind.
    ///
    /// # Errors
    /// Returns before bind for every invalid URL, policy, digest, path, or secret file.
    pub fn prepare(self) -> Result<PreparedServer, AppServerError> {
        if !self.listen_enabled {
            return Err(AppServerError::Config("--listen is required"));
        }
        let url = parse_listen_url(self.listen.as_deref())?;
        let host = url
            .host_str()
            .ok_or(AppServerError::Config("listen URL requires a host"))?
            .trim_start_matches('[')
            .trim_end_matches(']')
            .to_owned();
        let port = url.port().unwrap_or(0);
        let websocket_path = if url.path() == "/" {
            "/ws".to_owned()
        } else {
            url.path().to_owned()
        };
        let auth = AuthPolicy::prepare(&self)?;
        if !is_loopback_host(&host) && auth.is_none() {
            return Err(AppServerError::Config(
                "non-loopback listeners require websocket authentication",
            ));
        }
        Ok(PreparedServer {
            host,
            port,
            websocket_path,
            openai_api: self.openai_api,
            auth,
        })
    }
}

fn parse_listen_url(value: Option<&str>) -> Result<Url, AppServerError> {
    let url = Url::parse(value.unwrap_or("ws://127.0.0.1:0"))
        .map_err(|_| AppServerError::Config("invalid listen URL"))?;
    if url.scheme() != "ws" || url.username() != "" || url.password().is_some() {
        return Err(AppServerError::Config("listen URL must use plain ws"));
    }
    if url.query().is_some() || url.fragment().is_some() {
        return Err(AppServerError::Config(
            "listen URL cannot contain query or fragment",
        ));
    }
    if url.host_str().is_none() {
        return Err(AppServerError::Config("listen URL requires a host"));
    }
    Ok(url)
}

/// Returns whether a configured listen host is strictly loopback.
#[must_use]
pub fn is_loopback_host(host: &str) -> bool {
    let normalized = host.trim_start_matches('[').trim_end_matches(']');
    if normalized.eq_ignore_ascii_case("localhost") {
        return true;
    }
    normalized
        .parse::<IpAddr>()
        .is_ok_and(|address| address.is_loopback())
}
