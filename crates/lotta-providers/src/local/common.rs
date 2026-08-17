use lotta_runtime::RuntimeError;
use reqwest::{Client, Url, header::HeaderValue};

pub(crate) fn client(endpoint: &Url, credential: &str) -> Result<Client, RuntimeError> {
    if !crate::native::shared::endpoint_allowed(endpoint, credential_present(credential)) {
        return Err(invalid(
            "local provider endpoint requires HTTPS or an uncredentialed LAN URL",
        ));
    }
    Client::builder()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .map_err(|_| invalid("local provider client"))
}

pub(crate) fn credential(value: &str) -> Result<Option<HeaderValue>, RuntimeError> {
    if value.is_empty() || value == "not-needed" {
        return Ok(None);
    }
    let mut header = HeaderValue::from_str(value).map_err(|_| invalid("provider credential"))?;
    header.set_sensitive(true);
    Ok(Some(header))
}

pub(crate) fn endpoint(base: &Url, path: &str) -> Url {
    let mut value = base.clone();
    let mut root = value.path().trim_end_matches('/').to_owned();
    if root.ends_with("/v1") {
        root.truncate(root.len() - 3);
    }
    value.set_path(&format!("{root}/{path}"));
    value.set_query(None);
    value.set_fragment(None);
    value
}

pub(crate) fn invalid(context: &'static str) -> RuntimeError {
    RuntimeError::InvalidData {
        context: context.into(),
    }
}

fn credential_present(value: &str) -> bool {
    !value.is_empty() && value != "not-needed"
}
