//! Optional retries at the raw HTTP request boundary.
use std::{
    future::Future,
    hash::BuildHasher,
    time::{Duration, Instant},
};

#[cfg(any(
    feature = "openai",
    feature = "deepseek",
    feature = "azure-ai",
    feature = "anthropic",
    feature = "gemini"
))]
pub(crate) mod http;

/// Optional request retries before a response body stream starts.
///
/// Only HTTP 5xx and connection, timeout or network failures are retried. HTTP
/// 429 and local request construction errors return to the caller unchanged.
/// The default disables this layer; disable the client's model-level retry when
/// configuring a separate stream recovery owner.
///
/// # Example
/// ```
/// use adk_model::request_retry::RequestRetryConfig;
/// use std::time::Duration;
/// let policy = RequestRetryConfig { max_retries: 4, initial_delay: Duration::from_millis(200) };
/// assert_eq!(policy.max_retries, 4);
/// ```
#[derive(Clone, Copy, Debug)]
pub struct RequestRetryConfig {
    /// Additional sends after the initial HTTP request.
    pub max_retries: u32,
    /// Exponential backoff base, with ten percent jitter.
    pub initial_delay: Duration,
}
impl Default for RequestRetryConfig {
    fn default() -> Self {
        Self { max_retries: 0, initial_delay: Duration::from_millis(200) }
    }
}
impl RequestRetryConfig {
    fn delay(self, attempt: u32) -> Duration {
        let sample = std::collections::hash_map::RandomState::new().hash_one(attempt);
        let jitter = 0.9 + 0.2 * (sample as f64 / u64::MAX as f64);
        let factor = 2_f64.powi(i32::try_from(attempt.saturating_sub(1).min(31)).unwrap());
        self.initial_delay.mul_f64(factor * jitter)
    }
}

pub(crate) async fn execute<T, E, Op, Fut, Retry, Deadline>(
    policy: RequestRetryConfig,
    mut operation: Op,
    should_retry: Retry,
    retry_at: Deadline,
) -> Result<T, E>
where
    Op: FnMut() -> Fut,
    Fut: Future<Output = Result<T, E>>,
    Retry: Fn(&Result<T, E>) -> bool,
    Deadline: Fn(&Result<T, E>) -> Option<Instant>,
{
    let mut retries = 0;
    loop {
        let result = operation().await;
        if !should_retry(&result) || retries >= policy.max_retries {
            return result;
        }
        retries += 1;
        let Some(deadline) =
            retry_at(&result).or_else(|| Instant::now().checked_add(policy.delay(retries)))
        else {
            return result;
        };
        drop(result);
        tokio::time::sleep_until(tokio::time::Instant::from_std(deadline)).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn keeps_jitter_within_exponential_bounds() {
        let policy = RequestRetryConfig { max_retries: 4, ..Default::default() };
        for attempt in 1..=4 {
            let raw = 200 * (1 << (attempt - 1));
            let delay = policy.delay(attempt);
            assert!(delay >= Duration::from_millis(raw * 9 / 10));
            assert!(delay <= Duration::from_millis(raw * 11 / 10));
        }
        assert_eq!(RequestRetryConfig::default().max_retries, 0);
    }
}
