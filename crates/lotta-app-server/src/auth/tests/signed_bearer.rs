use crate::auth::test_support::{authorize, signed_policy, token, token_with_header};
use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use serde_json::{Value, json};

const SECRET: &[u8; 32] = b"01234567890123456789012345678901";
fn policy() -> super::AuthPolicy {
    signed_policy(SECRET)
}
fn ok(claims: &Value) -> bool {
    authorize(&policy(), &token(claims, SECRET), 60).is_ok()
}
fn bad(claims: &Value) -> bool {
    !ok(claims)
}

#[test]
fn accepts_valid_token() {
    assert!(ok(&json!({"exp":61})));
}
#[test]
fn rejects_missing_exp() {
    assert!(bad(&json!({})));
}
#[test]
fn rejects_wrong_issuer() {
    let p = policy_with(Some("good"), None);
    assert!(authorize(&p, &token(&json!({"exp":61,"iss":"bad"}), SECRET), 60).is_err());
}
#[test]
fn rejects_wrong_audience() {
    let p = policy_with(None, Some("good"));
    assert!(authorize(&p, &token(&json!({"exp":61,"aud":"bad"}), SECRET), 60).is_err());
}
#[test]
fn rejects_non_hs256() {
    let t = token_with_header(&json!({"alg":"none"}), &json!({"exp":61}), SECRET);
    assert!(authorize(&policy(), &t, 60).is_err());
}
#[test]
fn rejects_bad_signature() {
    let t = token(&json!({"exp":61}), b"abcdefghabcdefghabcdefghabcdefgh");
    assert!(authorize(&policy(), &t, 60).is_err());
}
#[test]
fn expiry_boundary_and_expires() {
    assert!(ok(&json!({"exp":30})));
    assert!(bad(&json!({"exp":29})));
}
#[test]
fn nbf_boundary_and_early() {
    assert!(ok(&json!({"exp":90,"nbf":90})));
    assert!(bad(&json!({"exp":91,"nbf":91})));
}
#[test]
fn rejects_float_and_unsafe_exp() {
    assert!(bad(&json!({"exp":60.5})));
    assert!(bad(&json!({"exp":9_007_199_254_740_992_i64})));
}
#[test]
fn rejects_malformed_nbf() {
    assert!(bad(&json!({"exp":61,"nbf":"soon"})));
}
#[test]
fn audience_string_and_list_are_accepted() {
    let p = policy_with(None, Some("good"));
    for aud in [json!("good"), json!(["other", "good"])] {
        let claims = json!({"exp":61,"aud":aud});
        assert!(authorize(&p, &token(&claims, SECRET), 60).is_ok());
    }
}
#[test]
fn malformed_audience_rejected_without_expected() {
    for aud in [json!(7), json!(["good", 7])] {
        assert!(bad(&json!({"exp":61,"aud":aud})));
    }
}
#[test]
fn oversized_audience_rejected_without_expected() {
    assert!(bad(&json!({"exp":61,"aud":vec!["x";65]})));
}
#[test]
fn exact_three_components_required() {
    for t in ["a.b", "a.b.c.d", ".."] {
        assert!(authorize(&policy(), t, 60).is_err());
    }
}
#[test]
fn rejects_padding_and_noncanonical_base64() {
    let mut t = token(&json!({"exp":61}), SECRET);
    t.push('=');
    assert!(authorize(&policy(), &t, 60).is_err());
    assert!(authorize(&policy(), "AB.AB.AB", 60).is_err());
}
#[test]
fn rejects_bad_json() {
    let h = URL_SAFE_NO_PAD.encode(br#"{"alg":"HS256"}"#);
    assert!(authorize(&policy(), &format!("{h}.eA.eA"), 60).is_err());
}
#[test]
fn rejects_oversize_components() {
    let t = format!("{}.eA.eA", "a".repeat(8193));
    assert!(authorize(&policy(), &t, 60).is_err());
}

fn policy_with(issuer: Option<&str>, audience: Option<&str>) -> super::AuthPolicy {
    use crate::auth::test_support::{signed_args, temp_file};
    let path = temp_file(SECRET);
    let mut args = signed_args(&path);
    args.ws_issuer = issuer.map(str::to_owned);
    args.ws_audience = audience.map(str::to_owned);
    super::AuthPolicy::prepare(&args).unwrap_or_else(|_| panic!("prepare"))
}
