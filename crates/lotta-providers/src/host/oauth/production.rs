use super::*;

/// Production monotonic clock with an arbitrary process-local origin.
#[derive(Debug)]
pub struct MonotonicOAuthClock {
    origin: Instant,
}

impl Default for MonotonicOAuthClock {
    fn default() -> Self {
        Self {
            origin: Instant::now(),
        }
    }
}

impl OAuthClock for MonotonicOAuthClock {
    fn now_seconds(&self) -> u64 {
        self.origin.elapsed().as_secs()
    }
}

/// Production bounded OAuth HTTP port.
pub struct ReqwestOAuthHttp {
    client: reqwest::Client,
}

impl fmt::Debug for ReqwestOAuthHttp {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("ReqwestOAuthHttp")
    }
}

impl ReqwestOAuthHttp {
    /// Builds a TLS-only client which ignores proxies and refuses redirects.
    pub fn new() -> Result<Self, OAuthError> {
        let timeout = Duration::from_secs(OAUTH_HTTP_TIMEOUT_SECONDS);
        let client = reqwest::Client::builder()
            .https_only(true)
            .no_proxy()
            .redirect(reqwest::redirect::Policy::none())
            .connect_timeout(timeout)
            .timeout(timeout)
            .build()
            .map_err(|_| OAuthError::Unavailable)?;
        Ok(Self { client })
    }

    fn send(request: reqwest::RequestBuilder) -> Result<OAuthHttpResponse, OAuthError> {
        let timeout = Duration::from_secs(OAUTH_HTTP_TIMEOUT_SECONDS);
        let operation = async move {
            let response = request.send().await.map_err(|_| OAuthError::Provider)?;
            let status = response.status().as_u16();
            let declared = response.content_length().unwrap_or(0);
            if declared > OAUTH_HTTP_RESPONSE_BYTES_MAX as u64 {
                return Err(OAuthError::Provider);
            }
            let bytes = response.bytes().await.map_err(|_| OAuthError::Provider)?;
            Ok((status, bytes))
        };
        let (status, bytes) = std::thread::scope(|scope| {
            scope
                .spawn(|| {
                    tokio::runtime::Builder::new_current_thread()
                        .enable_all()
                        .build()
                        .map_err(|_| OAuthError::Unavailable)?
                        .block_on(async { tokio::time::timeout(timeout, operation).await })
                        .map_err(|_| OAuthError::Provider)?
                })
                .join()
                .map_err(|_| OAuthError::Unavailable)?
        })?;
        if bytes.len() > OAUTH_HTTP_RESPONSE_BYTES_MAX {
            return Err(OAuthError::Provider);
        }
        let value: serde_json::Value =
            serde_json::from_slice(&bytes).map_err(|_| OAuthError::Provider)?;
        let object = value.as_object().ok_or(OAuthError::Provider)?;
        let mut fields = HashMap::with_capacity(object.len());
        for (name, value) in object {
            validate_text(name)?;
            let value = match value {
                serde_json::Value::String(value) => value.clone(),
                serde_json::Value::Number(value) => value.to_string(),
                _ => continue,
            };
            validate_text(&value)?;
            fields.insert(name.clone(), value);
        }
        Ok(OAuthHttpResponse { status, fields })
    }
}

impl OAuthHttp for ReqwestOAuthHttp {
    fn post_form(
        &self,
        url: &Url,
        fields: &[(&str, &str)],
    ) -> Result<OAuthHttpResponse, OAuthError> {
        validate_production_endpoint(url)?;
        Self::send(self.client.post(url.clone()).form(fields))
    }

    fn post_json(
        &self,
        url: &Url,
        fields: &[(&str, &str)],
    ) -> Result<OAuthHttpResponse, OAuthError> {
        validate_production_endpoint(url)?;
        let body: HashMap<&str, &str> = fields.iter().copied().collect();
        Self::send(self.client.post(url.clone()).json(&body))
    }
}

/// OS CSPRNG implementation for production.
#[derive(Debug, Default)]
pub struct OsOAuthRandom;

impl OAuthRandom for OsOAuthRandom {
    fn fill(&self, bytes: &mut [u8]) -> Result<(), OAuthError> {
        getrandom::fill(bytes).map_err(|_| OAuthError::Unavailable)
    }
}
