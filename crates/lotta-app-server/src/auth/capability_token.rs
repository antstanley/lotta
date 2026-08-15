use lotta_domain::Secret;
use sha2::{Digest, Sha256};
use subtle::ConstantTimeEq;

use crate::error::AppServerError;

/// Decodes exactly 64 hexadecimal characters into a SHA-256 digest.
///
/// # Errors
/// Returns a scrubbed configuration error for non-canonical input.
pub fn decode_digest(value: &str) -> Result<[u8; 32], AppServerError> {
    if value.len() != 64 {
        return Err(AppServerError::Config(
            "token digest must be 64 hexadecimal characters",
        ));
    }
    let mut digest = [0_u8; 32];
    for (index, pair) in value.as_bytes().chunks_exact(2).enumerate() {
        let high = decode_nibble(pair[0])?;
        let low = decode_nibble(pair[1])?;
        digest[index] = (high << 4) | low;
    }
    Ok(digest)
}

fn decode_nibble(value: u8) -> Result<u8, AppServerError> {
    match value {
        b'0'..=b'9' => Ok(value - b'0'),
        b'a'..=b'f' => Ok(value - b'a' + 10),
        b'A'..=b'F' => Ok(value - b'A' + 10),
        _ => Err(AppServerError::Config("token digest must be hexadecimal")),
    }
}

/// Hashes the candidate and compares fixed-size digests in constant time.
///
/// # Errors
/// Returns unauthorized when the digest does not match.
pub fn verify(candidate: &[u8], expected: &Secret<[u8; 32]>) -> Result<(), AppServerError> {
    let actual: [u8; 32] = Sha256::digest(candidate).into();
    let accepted = expected.expose_secret(|digest| digest.ct_eq(&actual).into());
    if accepted {
        Ok(())
    } else {
        Err(AppServerError::Unauthorized)
    }
}
