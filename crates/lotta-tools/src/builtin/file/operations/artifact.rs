use super::{
    FileError, FileState, IMAGE_BYTES_MAX, OUTPUT_BYTES_MAX, OperationControl, TEXT_FILE_BYTES_MAX,
    Value, artifact_relative, atomic_write, bounded, decoded_upper_bound, encoded_len,
    read_regular, string, workspace_relative,
};
use base64::{Engine as _, engine::general_purpose::STANDARD};
use serde_json::json;
use std::path::Path;

pub(super) fn view_image(
    state: &FileState,
    input: &Value,
    control: &OperationControl,
) -> Result<String, FileError> {
    control.check()?;
    let path = workspace_relative(state, string(input, "path")?)?;
    let mime = image_mime(&path)?;
    let metadata = state
        .workspace
        .symlink_metadata(&path)
        .map_err(|_| FileError::Tool)?;
    let length = usize::try_from(metadata.len()).map_err(|_| FileError::Tool)?;
    validate_image_preflight(length)?;
    let bytes = read_regular(&state.workspace, &path, IMAGE_BYTES_MAX, control)?;
    bounded(
        serde_json::to_string(&json!({"mime_type":mime,"data":STANDARD.encode(bytes)}))
            .map_err(|_| FileError::Tool)?,
    )
}

#[derive(Debug, PartialEq, Eq)]
enum ImagePreflightError {
    Source,
    Output,
}

fn validate_image_source(length: usize) -> Result<(), ImagePreflightError> {
    (length <= IMAGE_BYTES_MAX)
        .then_some(())
        .ok_or(ImagePreflightError::Source)
}

fn validate_image_output(length: usize) -> Result<(), ImagePreflightError> {
    let encoded = encoded_len(length).map_err(|_| ImagePreflightError::Output)?;
    encoded
        .checked_add(64)
        .filter(|value| *value <= OUTPUT_BYTES_MAX)
        .map(|_| ())
        .ok_or(ImagePreflightError::Output)
}

fn validate_image_preflight(length: usize) -> Result<(), FileError> {
    validate_image_source(length).map_err(|_| FileError::Tool)?;
    validate_image_output(length).map_err(|_| FileError::Tool)
}

pub(super) fn image_mime(path: &Path) -> Result<&'static str, FileError> {
    match path
        .extension()
        .and_then(|value| value.to_str())
        .map(str::to_ascii_lowercase)
        .as_deref()
    {
        Some("png") => Ok("image/png"),
        Some("jpg" | "jpeg") => Ok("image/jpeg"),
        Some("gif") => Ok("image/gif"),
        Some("webp") => Ok("image/webp"),
        Some("bmp") => Ok("image/bmp"),
        Some("heic") => Ok("image/heic"),
        Some("heif") => Ok("image/heif"),
        _ => Err(FileError::Tool),
    }
}

pub(super) fn read_artifact(
    state: &FileState,
    input: &Value,
    control: &OperationControl,
) -> Result<String, FileError> {
    control.check()?;
    let path = artifact_relative(state, string(input, "path")?)?;
    let encoding = input
        .get("encoding")
        .and_then(Value::as_str)
        .unwrap_or("utf8");
    let maximum = match encoding {
        "utf8" => OUTPUT_BYTES_MAX,
        "base64" => (OUTPUT_BYTES_MAX / 4) * 3,
        _ => return Err(FileError::Tool),
    };
    let bytes = read_regular(&state.artifacts, &path, maximum, control)?;
    match input
        .get("encoding")
        .and_then(Value::as_str)
        .unwrap_or("utf8")
    {
        "utf8" => bounded(String::from_utf8(bytes).map_err(|_| FileError::Tool)?),
        "base64" => bounded(STANDARD.encode(bytes)),
        _ => Err(FileError::Tool),
    }
}

pub(super) fn write_artifact(
    state: &FileState,
    input: &Value,
    control: &OperationControl,
) -> Result<String, FileError> {
    let path = artifact_relative(state, string(input, "path")?)?;
    let content = string(input, "content")?;
    let bytes = match input
        .get("encoding")
        .and_then(Value::as_str)
        .unwrap_or("utf8")
    {
        "utf8" => content.as_bytes().to_vec(),
        "base64" => {
            if decoded_upper_bound(content.len())? > TEXT_FILE_BYTES_MAX {
                return Err(FileError::Tool);
            }
            STANDARD.decode(content).map_err(|_| FileError::Tool)?
        }
        _ => return Err(FileError::Tool),
    };
    if bytes.len() > TEXT_FILE_BYTES_MAX {
        return Err(FileError::Tool);
    }
    let _guard = state.mutations.lock().map_err(|_| FileError::Tool)?;
    control.check()?;
    atomic_write(&state.artifacts, &path, &bytes)?;
    Ok("Artifact written successfully.".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn image_source_boundaries() {
        assert_eq!(validate_image_source(IMAGE_BYTES_MAX - 1), Ok(()));
        assert_eq!(validate_image_source(IMAGE_BYTES_MAX), Ok(()));
        assert_eq!(
            validate_image_source(IMAGE_BYTES_MAX + 1),
            Err(ImagePreflightError::Source)
        );
    }

    #[test]
    fn image_encoded_output_boundaries() {
        let raw = ((OUTPUT_BYTES_MAX - 64) / 4) * 3;
        assert_eq!(validate_image_output(raw - 1), Ok(()));
        assert_eq!(validate_image_output(raw), Ok(()));
        assert_eq!(
            validate_image_output(raw + 1),
            Err(ImagePreflightError::Output)
        );
    }

    #[test]
    fn image_preflight_boundaries_identify_ordering() {
        assert_eq!(
            validate_image_source(IMAGE_BYTES_MAX + 1),
            Err(ImagePreflightError::Source)
        );
        assert_eq!(
            validate_image_output(IMAGE_BYTES_MAX),
            Err(ImagePreflightError::Output)
        );
        assert!(validate_image_preflight(IMAGE_BYTES_MAX).is_err());
    }
}
