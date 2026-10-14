//! Bounded provider error codes separate from provider-controlled display text.

use adk_core::AdkError;

fn valid_code(code: &str) -> bool {
    !code.is_empty()
        && code.len() <= 128
        && code.bytes().all(|byte| byte.is_ascii_alphanumeric() || b"_.-".contains(&byte))
}

pub(crate) fn with_code(mut error: AdkError, code: Option<&str>) -> AdkError {
    if let Some(code) = code.filter(|code| valid_code(code)) {
        error.details.metadata.insert("provider_error_code".into(), serde_json::json!(code));
    }
    error
}

#[cfg(any(feature = "azure-ai", feature = "deepseek", feature = "openai"))]
pub(crate) fn from_body(error: AdkError, body: &str) -> AdkError {
    if body.len() > 64 * 1024 {
        return error;
    }
    let Ok(body) = serde_json::from_str::<serde_json::Value>(body) else {
        return error;
    };
    let code = ["/error/code", "/error/status"].into_iter().find_map(|path| {
        body.pointer(path).and_then(serde_json::Value::as_str).filter(|code| valid_code(code))
    });
    with_code(error, code)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn error() -> AdkError {
        AdkError::model("provider failure")
    }

    fn code(error: &AdkError) -> Option<&str> {
        error.details.metadata.get("provider_error_code").and_then(serde_json::Value::as_str)
    }

    #[test]
    fn bounds_typed_codes() {
        let oversized = "x".repeat(129);
        for value in ["", "bad code", "line\nbreak", "é", oversized.as_str()] {
            assert_eq!(code(&with_code(error(), Some(value))), None, "{value:?}");
        }
        let boundary = "x".repeat(128);
        assert_eq!(code(&with_code(error(), Some(&boundary))), Some(boundary.as_str()));
        assert_eq!(
            code(&with_code(error(), Some("RESOURCE_EXHAUSTED"))),
            Some("RESOURCE_EXHAUSTED")
        );
    }

    #[cfg(any(feature = "azure-ai", feature = "deepseek", feature = "openai"))]
    #[test]
    fn parses_only_bounded_error_objects() {
        assert_eq!(
            code(&from_body(error(), r#"{"error":{"code":"quota.exhausted"}}"#)),
            Some("quota.exhausted")
        );
        assert_eq!(
            code(&from_body(error(), r#"{"error":{"status":"RESOURCE_EXHAUSTED"}}"#)),
            Some("RESOURCE_EXHAUSTED")
        );
        assert_eq!(
            code(&from_body(error(), r#"{"error":{"code":12,"status":"RESOURCE_EXHAUSTED"}}"#)),
            Some("RESOURCE_EXHAUSTED")
        );
        for body in [
            r#"{"error":{"code":"bad code"}}"#,
            r#"{"error":{"code":12}}"#,
            r#"{"message":"error.code=secret"}"#,
            r#"prefix {"error":{"code":"secret"}}"#,
            r#"{"error":{"code":"secret"}} trailing"#,
        ] {
            assert_eq!(code(&from_body(error(), body)), None, "{body}");
        }
        let oversized_code = format!(r#"{{"error":{{"code":"{}"}}}}"#, "x".repeat(129));
        assert_eq!(code(&from_body(error(), &oversized_code)), None);
        let oversized_body =
            format!(r#"{{"error":{{"code":"valid","message":"{}"}}}}"#, "x".repeat(64 * 1024));
        assert_eq!(code(&from_body(error(), &oversized_body)), None);
    }
}
