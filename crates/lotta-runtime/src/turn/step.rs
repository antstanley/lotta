use super::ToolResultRecord;
use crate::RuntimeError;
use crate::boundary::ProviderText;
use crate::ports::{ProviderContent, ProviderContentPart, ProviderMessage, ProviderMessageRole};

pub(crate) fn tool_message(result: &ToolResultRecord) -> Result<ProviderMessage, RuntimeError> {
    let serialized =
        serde_json::to_string(&result.outcome).map_err(|_| RuntimeError::InvalidData {
            context: "turn tool outcome serialization".into(),
        })?;
    let text = ProviderText::new(serialized)?;
    let content = ProviderContent::new(vec![ProviderContentPart::Text(text)]).map_err(|_| {
        RuntimeError::LimitExceeded {
            context: "PROVIDER_CONTENT_PARTS_MAX".into(),
        }
    })?;
    Ok(ProviderMessage {
        role: ProviderMessageRole::Tool,
        content,
        tool_call_id: Some(result.call_id.clone()),
    })
}
