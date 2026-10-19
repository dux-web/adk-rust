//! Structured upstream diagnostics require application-owned redaction before display.
use std::fmt;

/// Selected provider error fields, retained outside public error formatting.
///
/// Values are untrusted and may echo credentials. Applications must redact their
/// captured authentication values before displaying or persisting these fields.
/// This type intentionally does not implement serialization.
#[derive(Clone, Default)]
pub struct ProviderErrorDetails {
    /// Provider error code, distinct from ADK's static diagnostic code.
    pub code: Option<String>,
    /// Provider error category.
    pub kind: Option<String>,
    /// Provider error message.
    pub message: Option<String>,
    /// Parameter identified by the provider.
    pub param: Option<String>,
    /// Upstream HTTP status, when an HTTP failure supplied it.
    pub status: Option<u16>,
    /// Provider correlation identifier, when present in the error object.
    pub request_id: Option<String>,
}

impl fmt::Debug for ProviderErrorDetails {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("ProviderErrorDetails { diagnostics: redacted }")
    }
}

impl fmt::Display for ProviderErrorDetails {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("upstream provider error (diagnostics redacted)")
    }
}

impl std::error::Error for ProviderErrorDetails {}
