use super::handshake::SidecarSessionPolicy;
use super::*;
use serde_json::json;

pub(super) fn owner(label: &str) -> SidecarOwnerIdentity {
    SidecarOwnerIdentity::new(label, "runtime", "conversation").expect("valid owner")
}

pub(super) fn envelope(kind: SidecarEnvelopeKind) -> SidecarEnvelope {
    SidecarEnvelope {
        version: SIDECAR_PROTOCOL_VERSION,
        owner: owner("agent"),
        capability: SidecarCapability::Mod,
        timeout_ms: 1_000,
        request_id: "request".into(),
        correlation_id: None,
        kind,
        payload: json!({"value": true}),
    }
}

pub(super) fn policy() -> SidecarSessionPolicy {
    SidecarSessionPolicy::new(
        SIDECAR_PROTOCOL_VERSION,
        owner("agent"),
        [SidecarCapability::Mod],
        SIDECAR_TIMEOUT_MS_MAX,
        SidecarFrameLimit::mod_host().tightened(16 * 1024),
    )
}

pub(super) fn wire(values: &[SidecarEnvelope]) -> Vec<u8> {
    let mut output = Vec::new();
    for value in values {
        let payload = serde_json::to_vec(value).expect("encode envelope");
        output.extend_from_slice(&u32::try_from(payload.len()).unwrap().to_be_bytes());
        output.extend_from_slice(&payload);
    }
    output
}
