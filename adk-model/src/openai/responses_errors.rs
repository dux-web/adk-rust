//! Responses HTTP error normalization and retry metadata.

use adk_core::{AdkError, ErrorCategory, ErrorComponent};
use async_openai::error::{ApiError, ApiErrorResponse, OpenAIError, WrappedError};
use chrono::{DateTime, Datelike, NaiveDateTime, Utc};
use std::{
    fmt,
    time::{Duration, Instant},
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum RetryAfter {
    Fallback,
    Delay(u64),
    Unsupported,
}

struct HttpError {
    error: ApiErrorResponse,
    retry_after: RetryAfter,
}

impl fmt::Debug for HttpError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("HttpError")
            .field("status", &self.error.status_code.as_u16())
            .field("retry_after", &self.retry_after)
            .finish()
    }
}

impl fmt::Display for HttpError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "OpenAI Responses HTTP {}", self.error.status_code.as_u16())
    }
}

impl std::error::Error for HttpError {}

/// Decode a failed HTTP response before the SDK can replace its status with a
/// body-deserialization error. The original `Retry-After` header is captured
/// before consuming the body.
pub(super) async fn normalize_response(
    response: reqwest_openai::Response,
    open_responses_mode: bool,
) -> Result<reqwest_openai::Response, OpenAIError> {
    let status = response.status();
    let retry_after = retry_after(response.headers());
    let bytes = response.bytes().await.unwrap_or_default();
    if let Ok(wrapped) = serde_json::from_slice::<WrappedError>(&bytes) {
        return Err(http_error(
            ApiErrorResponse { status_code: status, api_error: wrapped.error },
            retry_after,
        ));
    }
    if open_responses_mode
        && let Ok(value) = serde_json::from_slice::<serde_json::Value>(&bytes)
        && value.get("error").is_none()
        && let Some(detail) = value.get("detail").and_then(serde_json::Value::as_str)
    {
        return Err(http_error(
            ApiErrorResponse {
                status_code: status,
                api_error: ApiError {
                    message: detail.to_owned(),
                    r#type: None,
                    param: None,
                    code: value.get("code").and_then(serde_json::Value::as_str).map(str::to_owned),
                },
            },
            retry_after,
        ));
    }
    Err(http_error(
        ApiErrorResponse {
            status_code: status,
            api_error: ApiError {
                message: format!("HTTP {status}"),
                r#type: None,
                param: None,
                code: None,
            },
        },
        retry_after,
    ))
}

fn retry_after(headers: &http::HeaderMap) -> RetryAfter {
    parse_delay(
        headers.get(http::header::RETRY_AFTER).and_then(|value| value.to_str().ok()),
        Utc::now(),
        Instant::now(),
    )
}

fn parse_delay(value: Option<&str>, now: DateTime<Utc>, started: Instant) -> RetryAfter {
    let Some(value) = value.map(str::trim).filter(|value| !value.is_empty()) else {
        return RetryAfter::Fallback;
    };
    let delay = if value.bytes().all(|byte| byte.is_ascii_digit()) {
        let Ok(seconds) = value.parse::<u64>() else {
            return RetryAfter::Unsupported;
        };
        Duration::from_secs(seconds)
    } else {
        let Some(date) = parse_date(value, now) else {
            return RetryAfter::Fallback;
        };
        date.signed_duration_since(now).to_std().unwrap_or(Duration::ZERO)
    };
    // A date can contain fractional seconds relative to `now`; round up to
    // the retry hint's millisecond precision, never earlier than requested.
    let Ok(millis) = u64::try_from(delay.as_nanos().div_ceil(1_000_000)) else {
        return RetryAfter::Unsupported;
    };
    if started.checked_add(Duration::from_millis(millis)).is_none() {
        return RetryAfter::Unsupported;
    }
    RetryAfter::Delay(millis)
}

fn parse_date(value: &str, now: DateTime<Utc>) -> Option<DateTime<Utc>> {
    if let Ok(date) = DateTime::parse_from_rfc2822(value) {
        return Some(date.with_timezone(&Utc));
    }
    if let Ok(date) = NaiveDateTime::parse_from_str(value, "%a %b %e %H:%M:%S %Y") {
        return Some(date.and_utc());
    }
    let mut parsed = chrono::format::Parsed::new();
    chrono::format::parse(
        &mut parsed,
        value,
        chrono::format::StrftimeItems::new("%A, %d-%b-%y %H:%M:%S GMT"),
    )
    .ok()?;
    // Resolve the obsolete two-digit year before validating the weekday.
    let cutoff = now.year() + 50;
    let mut year = cutoff.div_euclid(100) * 100 + parsed.year_mod_100?;
    if year > cutoff {
        year -= 100;
    }
    let weekday = parsed.weekday.take();
    parsed.set_year(i64::from(year)).ok()?;
    let date = parsed.to_naive_datetime_with_offset(0).ok()?.and_utc();
    if (date.year(), date.month(), date.day(), date.time())
        > (cutoff, now.month(), now.day(), now.time())
    {
        parsed.year = Some(year - 100);
    }
    parsed.weekday = weekday;
    Some(parsed.to_naive_datetime_with_offset(0).ok()?.and_utc())
}

fn http_error(error: ApiErrorResponse, retry_after: RetryAfter) -> OpenAIError {
    match retry_after {
        RetryAfter::Fallback => OpenAIError::ApiError(error),
        _ => OpenAIError::Boxed(Box::new(HttpError { error, retry_after })),
    }
}

pub(super) fn mapped_http_error(error: &OpenAIError) -> Option<AdkError> {
    let OpenAIError::Boxed(error) = error else {
        return None;
    };
    let error = error.downcast_ref::<HttpError>()?;
    let mut mapped = map_api_error(&error.error);
    match error.retry_after {
        RetryAfter::Delay(millis)
            if Instant::now().checked_add(Duration::from_millis(millis)).is_some() =>
        {
            if mapped.retry.should_retry {
                mapped.retry.retry_after_ms = Some(millis);
            }
        }
        RetryAfter::Delay(_) | RetryAfter::Unsupported => {
            // An unsupported valid delay cannot authorize an earlier fallback retry.
            mapped.retry.should_retry = false;
            mapped.retry.retry_after_ms = None;
        }
        RetryAfter::Fallback => {}
    }
    Some(mapped)
}

pub(super) fn map_api_error(api_err: &ApiErrorResponse) -> AdkError {
    let status_code = api_err.status_code.as_u16();
    let (category, code) = match status_code {
        401 => (ErrorCategory::Unauthorized, "model.openai_responses.unauthorized"),
        403 => (ErrorCategory::Forbidden, "model.openai_responses.forbidden"),
        404 => (ErrorCategory::NotFound, "model.openai_responses.not_found"),
        408 => (ErrorCategory::Timeout, "model.openai_responses.timeout"),
        429 => (ErrorCategory::RateLimited, "model.openai_responses.rate_limited"),
        500 | 502 | 503 | 504 | 529 => {
            (ErrorCategory::Unavailable, "model.openai_responses.unavailable")
        }
        400..=499 => (ErrorCategory::InvalidInput, "model.openai_responses.invalid_request"),
        _ => (ErrorCategory::Internal, "model.openai_responses.api_error"),
    };
    let mut mapped = AdkError::new(
        ErrorComponent::Model,
        category,
        code,
        format!("OpenAI Responses API error: {api_err}"),
    )
    .with_provider("openai-responses")
    .with_upstream_status(status_code);
    if let Some(provider_code) = api_err.api_error.code.as_deref().filter(|code| {
        !code.is_empty()
            && code.len() <= 128
            && code.bytes().all(|byte| byte.is_ascii_alphanumeric() || b"_.-".contains(&byte))
    }) {
        mapped.details.metadata.insert(
            "provider_error_code".to_string(),
            serde_json::Value::String(provider_code.to_string()),
        );
    }
    mapped
}

#[cfg(test)]
mod tests {
    use super::*;

    fn now() -> DateTime<Utc> {
        "2026-10-12T00:00:00Z".parse().unwrap()
    }

    fn parse(value: Option<&str>) -> RetryAfter {
        parse_delay(value, now(), Instant::now())
    }

    #[test]
    fn parses_seconds_and_dates() {
        assert_eq!(parse(Some(" 12 ")), RetryAfter::Delay(12_000));
        assert_eq!(parse(Some("0")), RetryAfter::Delay(0));
        for value in [
            "Mon, 12 Oct 2026 00:00:12 GMT",
            "Monday, 12-Oct-26 00:00:12 GMT",
            "Mon Oct 12 00:00:12 2026",
        ] {
            assert_eq!(parse(Some(value)), RetryAfter::Delay(12_000), "{value}");
        }
        assert_eq!(parse(Some("Sun, 11 Oct 2026 23:59:59 GMT")), RetryAfter::Delay(0));
    }

    #[test]
    fn rounds_fractional_date_delays_up() {
        let now = "2026-10-12T00:00:00.999500Z".parse().unwrap();
        assert_eq!(
            parse_delay(Some("Mon, 12 Oct 2026 00:00:01 GMT"), now, Instant::now()),
            RetryAfter::Delay(1)
        );
    }

    #[test]
    fn falls_back_for_invalid_values() {
        for value in [None, Some(""), Some("invalid"), Some("-1"), Some("+1"), Some("1.5")] {
            assert_eq!(parse(value), RetryAfter::Fallback, "{value:?}");
        }
    }

    #[test]
    fn rejects_unrepresentable_delays() {
        for value in ["18446744073709551616", "18446744073709551615"] {
            assert_eq!(parse(Some(value)), RetryAfter::Unsupported);
        }
    }

    #[test]
    fn preserves_safe_provider_codes() {
        let error = ApiErrorResponse {
            status_code: reqwest_openai::StatusCode::BAD_REQUEST,
            api_error: ApiError {
                message: "invalid".into(),
                r#type: None,
                param: None,
                code: Some("invalid_schema".into()),
            },
        };
        let mapped = map_api_error(&error);
        assert_eq!(mapped.details.upstream_status_code, Some(400));
        assert_eq!(mapped.details.metadata["provider_error_code"], "invalid_schema");
    }

    #[test]
    fn drops_unsafe_provider_codes() {
        let error = ApiErrorResponse {
            status_code: reqwest_openai::StatusCode::BAD_REQUEST,
            api_error: ApiError {
                message: "invalid".into(),
                r#type: None,
                param: None,
                code: Some("secret token\nvalue".into()),
            },
        };
        let mapped = map_api_error(&error);
        assert!(!mapped.details.metadata.contains_key("provider_error_code"));
    }
}
