use adk_core::{ContextCacheConfig, Event};
use serde::{Deserialize, Serialize};

/// Identifies one cacheable context.
///
/// A provider cache is only valid for the material it was created from. Keying by the
/// model, the agent, and a digest of that material means a cache is never attached to a
/// request built from something else — previously a single Runner-wide name was reused
/// across sessions and across whichever sub-agent was selected.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) struct CacheKey {
    /// Model the cache was created against; a cache is provider- and model-specific.
    model: String,
    /// Agent whose instruction the cache holds.
    agent: String,
    /// Canonical digest of the instruction and tool material.
    material: String,
}

impl CacheKey {
    /// Build a key from the exact material a cache will be created from.
    pub(crate) fn new(model: &str, agent: &str, instruction: &str, tool_names: &[String]) -> Self {
        // Readable and canonical rather than hashed: tool order cannot change the key,
        // and a mismatch can be diagnosed by looking at it.
        let mut tools = tool_names.to_vec();
        tools.sort();
        Self {
            model: model.to_string(),
            agent: agent.to_string(),
            material: format!("{instruction}\u{1f}{}", tools.join(",")),
        }
    }
}

/// One cached context and how many invocations have used it.
#[derive(Debug, Clone)]
struct CacheEntry {
    name: String,
    invocation_count: u32,
}

/// Internal cache lifecycle manager.
///
/// Tracks a bounded map of caches keyed by [`CacheKey`], so a cache is reused only for
/// byte-equivalent material on the same model and agent, and determines when caching
/// should be attempted or refreshed based on [`ContextCacheConfig`] settings.
pub(crate) struct CacheManager {
    config: ContextCacheConfig,
    /// Live caches, keyed by the material they hold.
    entries: std::collections::HashMap<CacheKey, CacheEntry>,
    /// Insertion order, used to evict the oldest entry when at capacity.
    order: std::collections::VecDeque<CacheKey>,
}

/// Number of distinct cached contexts held at once.
///
/// A bound matters because the key includes the agent and its instruction: a deployment
/// with many agents, or instructions that vary, would otherwise accumulate provider-side
/// caches for the lifetime of the process.
const MAX_CACHE_ENTRIES: usize = 16;

impl CacheManager {
    pub(crate) fn new(config: ContextCacheConfig) -> Self {
        Self {
            config,
            entries: std::collections::HashMap::new(),
            order: std::collections::VecDeque::new(),
        }
    }

    /// Check if caching should be attempted based on config.
    ///
    /// Returns `false` when `min_tokens` or `ttl_seconds` is zero,
    /// effectively disabling the cache lifecycle.
    ///
    /// Note: The `min_tokens` threshold is enforced server-side by the
    /// provider (e.g., Gemini rejects cache creation for small contexts).
    /// A zero value here acts as a kill-switch for the entire lifecycle.
    pub(crate) fn is_enabled(&self) -> bool {
        self.config.min_tokens > 0 && self.config.ttl_seconds > 0
    }

    /// The cache for exactly this material, if one is live.
    #[cfg(test)]
    pub(crate) fn cache_name_for(&self, key: &CacheKey) -> Option<&str> {
        self.entries.get(key).map(|entry| entry.name.as_str())
    }

    /// Whether the cache for this material needs recreating.
    ///
    /// True when nothing is cached for it, or when the entry has served
    /// `cache_intervals` invocations.
    pub(crate) fn needs_refresh(&self, key: &CacheKey) -> bool {
        match self.entries.get(key) {
            None => true,
            Some(entry) => entry.invocation_count >= self.config.cache_intervals,
        }
    }

    /// Record an invocation against this material and return its cache name.
    pub(crate) fn record_invocation(&mut self, key: &CacheKey) -> Option<&str> {
        let entry = self.entries.get_mut(key)?;
        entry.invocation_count += 1;
        Some(entry.name.as_str())
    }

    /// Store a freshly created cache, returning any name it replaces.
    ///
    /// The replaced name is returned so the caller can delete it provider-side rather
    /// than leaking it. Storing at capacity evicts the oldest entry, whose name is
    /// returned for the same reason.
    pub(crate) fn store(&mut self, key: CacheKey, name: String) -> Vec<String> {
        let mut replaced = Vec::new();

        if let Some(previous) =
            self.entries.insert(key.clone(), CacheEntry { name, invocation_count: 0 })
        {
            replaced.push(previous.name);
        } else {
            self.order.push_back(key);
        }

        while self.order.len() > MAX_CACHE_ENTRIES {
            if let Some(oldest) = self.order.pop_front()
                && let Some(evicted) = self.entries.remove(&oldest)
            {
                replaced.push(evicted.name);
            }
        }

        replaced
    }

    /// Number of caches currently held.
    #[cfg(test)]
    pub(crate) fn len(&self) -> usize {
        self.entries.len()
    }
}

/// Metrics computed from session event history.
///
/// All ratio fields are percentages in the range `[0.0, 100.0]`.
/// When there are no events with usage metadata, all fields are zero.
///
/// # Example
///
/// ```rust,ignore
/// use adk_runner::CachePerformanceAnalyzer;
///
/// let events = session.events();
/// let metrics = CachePerformanceAnalyzer::analyze(&events);
/// println!("Cache hit ratio: {:.1}%", metrics.cache_hit_ratio);
/// ```
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct CacheMetrics {
    /// Total requests with `UsageMetadata`.
    pub total_requests: u32,
    /// Requests where `cache_read_input_token_count > 0`.
    pub requests_with_cache_hits: u32,
    /// Sum of all `prompt_token_count` values.
    pub total_prompt_tokens: i64,
    /// Sum of all `cache_read_input_token_count` values.
    pub total_cache_read_tokens: i64,
    /// Sum of all `cache_creation_input_token_count` values.
    pub total_cache_creation_tokens: i64,
    /// `total_cache_read_tokens / total_prompt_tokens * 100`.
    pub cache_hit_ratio: f64,
    /// `requests_with_cache_hits / total_requests * 100`.
    pub cache_utilization_ratio: f64,
    /// `total_cache_read_tokens / total_requests`.
    pub avg_cached_tokens_per_request: f64,
}

/// Utility for computing cache effectiveness metrics from session events.
///
/// This is a stateless analyzer — call [`CachePerformanceAnalyzer::analyze`]
/// with any slice of events to get a [`CacheMetrics`] snapshot.
///
/// # Example
///
/// ```rust,ignore
/// use adk_runner::CachePerformanceAnalyzer;
///
/// let metrics = CachePerformanceAnalyzer::analyze(&events);
/// println!("Hit ratio: {:.1}%, Utilization: {:.1}%",
///     metrics.cache_hit_ratio, metrics.cache_utilization_ratio);
/// ```
pub struct CachePerformanceAnalyzer;

impl CachePerformanceAnalyzer {
    /// Analyze cache performance from a slice of events.
    ///
    /// Iterates over all events, extracts `usage_metadata` from LLM responses,
    /// and computes aggregate cache metrics. Events without `usage_metadata`
    /// are skipped. An empty slice returns zeroed metrics.
    pub fn analyze(events: &[Event]) -> CacheMetrics {
        let mut metrics = CacheMetrics::default();

        for event in events {
            let Some(ref usage) = event.llm_response.usage_metadata else {
                continue;
            };

            metrics.total_requests += 1;
            metrics.total_prompt_tokens += i64::from(usage.prompt_token_count);

            let cache_read = usage.cache_read_input_token_count.unwrap_or(0);
            metrics.total_cache_read_tokens += i64::from(cache_read);

            if cache_read > 0 {
                metrics.requests_with_cache_hits += 1;
            }

            let cache_creation = usage.cache_creation_input_token_count.unwrap_or(0);
            metrics.total_cache_creation_tokens += i64::from(cache_creation);
        }

        if metrics.total_prompt_tokens > 0 {
            metrics.cache_hit_ratio =
                metrics.total_cache_read_tokens as f64 / metrics.total_prompt_tokens as f64 * 100.0;
        }
        if metrics.total_requests > 0 {
            metrics.cache_utilization_ratio =
                metrics.requests_with_cache_hits as f64 / metrics.total_requests as f64 * 100.0;
            metrics.avg_cached_tokens_per_request =
                metrics.total_cache_read_tokens as f64 / metrics.total_requests as f64;
        }

        metrics
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn default_config() -> ContextCacheConfig {
        ContextCacheConfig { min_tokens: 4096, ttl_seconds: 600, cache_intervals: 3 }
    }

    fn key(agent: &str, instruction: &str) -> CacheKey {
        CacheKey::new("gemini-2.5-flash", agent, instruction, &[])
    }

    #[test]
    fn a_new_manager_holds_nothing() {
        let cm = CacheManager::new(default_config());
        assert_eq!(cm.len(), 0);
        assert!(cm.cache_name_for(&key("a", "be helpful")).is_none());
    }

    #[test]
    fn test_is_enabled_with_valid_config() {
        let cm = CacheManager::new(default_config());
        assert!(cm.is_enabled());
    }

    #[test]
    fn test_is_enabled_false_when_min_tokens_zero() {
        let config = ContextCacheConfig { min_tokens: 0, ttl_seconds: 600, cache_intervals: 3 };
        assert!(!CacheManager::new(config).is_enabled());
    }

    #[test]
    fn test_is_enabled_false_when_ttl_zero() {
        let config = ContextCacheConfig { min_tokens: 4096, ttl_seconds: 0, cache_intervals: 3 };
        assert!(!CacheManager::new(config).is_enabled());
    }

    #[test]
    fn unknown_material_needs_a_cache() {
        let cm = CacheManager::new(default_config());
        assert!(cm.needs_refresh(&key("a", "be helpful")));
    }

    #[test]
    fn a_stored_cache_is_reused_until_its_interval_elapses() {
        let mut cm = CacheManager::new(default_config());
        let k = key("a", "be helpful");
        cm.store(k.clone(), "cachedContents/1".to_string());

        assert!(!cm.needs_refresh(&k));
        assert_eq!(cm.record_invocation(&k), Some("cachedContents/1"));
        assert_eq!(cm.record_invocation(&k), Some("cachedContents/1"));
        assert!(!cm.needs_refresh(&k));
        assert_eq!(cm.record_invocation(&k), Some("cachedContents/1"));
        assert!(cm.needs_refresh(&k), "three invocations reaches cache_intervals");
    }

    #[test]
    fn different_agents_do_not_share_a_cache() {
        // A cache made while one sub-agent was selected must not be attached to a
        // request for another.
        let mut cm = CacheManager::new(default_config());
        cm.store(key("planner", "plan carefully"), "cachedContents/planner".to_string());

        assert!(cm.cache_name_for(&key("writer", "plan carefully")).is_none());
        assert!(cm.needs_refresh(&key("writer", "plan carefully")));
    }

    #[test]
    fn changed_instruction_material_invalidates_reuse() {
        let mut cm = CacheManager::new(default_config());
        cm.store(key("a", "be helpful"), "cachedContents/1".to_string());

        assert!(
            cm.needs_refresh(&key("a", "be terse")),
            "a cache must not be reused for different instruction material"
        );
    }

    #[test]
    fn different_models_do_not_share_a_cache() {
        let mut cm = CacheManager::new(default_config());
        cm.store(
            CacheKey::new("gemini-2.5-flash", "a", "be helpful", &[]),
            "cachedContents/flash".to_string(),
        );
        let other = CacheKey::new("gemini-3-pro", "a", "be helpful", &[]);
        assert!(cm.cache_name_for(&other).is_none());
    }

    #[test]
    fn tool_order_does_not_change_identity() {
        let one = CacheKey::new("m", "a", "i", &["b".to_string(), "a".to_string()]);
        let two = CacheKey::new("m", "a", "i", &["a".to_string(), "b".to_string()]);
        assert_eq!(one, two);
    }

    #[test]
    fn storing_the_same_material_twice_reports_the_replaced_name() {
        let mut cm = CacheManager::new(default_config());
        let k = key("a", "be helpful");
        assert!(cm.store(k.clone(), "cachedContents/1".to_string()).is_empty());

        let replaced = cm.store(k, "cachedContents/2".to_string());
        assert_eq!(
            replaced,
            vec!["cachedContents/1".to_string()],
            "the superseded cache must be reported so it can be deleted provider-side"
        );
        assert_eq!(cm.len(), 1);
    }

    #[test]
    fn the_map_is_bounded_and_reports_evictions() {
        let mut cm = CacheManager::new(default_config());
        let mut evicted = Vec::new();
        for index in 0..(MAX_CACHE_ENTRIES + 4) {
            evicted.extend(cm.store(key(&format!("agent-{index}"), "i"), format!("cache/{index}")));
        }

        assert_eq!(cm.len(), MAX_CACHE_ENTRIES, "the cache map must stay bounded");
        assert_eq!(evicted.len(), 4, "each eviction must be reported for deletion");
        assert_eq!(evicted[0], "cache/0", "the oldest entry is evicted first");
    }

    #[test]
    fn a_full_lifecycle_stays_keyed() {
        let mut cm = CacheManager::new(ContextCacheConfig {
            min_tokens: 1024,
            ttl_seconds: 300,
            cache_intervals: 2,
        });
        let k = key("a", "be helpful");

        assert!(cm.is_enabled());
        assert!(cm.needs_refresh(&k), "nothing is cached yet");

        cm.store(k.clone(), "cachedContents/first".to_string());
        assert_eq!(cm.record_invocation(&k), Some("cachedContents/first"));
        assert!(!cm.needs_refresh(&k));
        assert_eq!(cm.record_invocation(&k), Some("cachedContents/first"));
        assert!(cm.needs_refresh(&k), "two invocations reaches cache_intervals");

        let replaced = cm.store(k.clone(), "cachedContents/second".to_string());
        assert_eq!(replaced, vec!["cachedContents/first".to_string()]);
        assert!(!cm.needs_refresh(&k), "a fresh cache resets the interval");
    }

    use adk_core::{LlmResponse, UsageMetadata};

    fn event_with_usage(
        prompt: i32,
        candidates: i32,
        cache_read: Option<i32>,
        cache_creation: Option<i32>,
    ) -> Event {
        let mut event = Event::new("test-invocation");
        event.llm_response = LlmResponse {
            usage_metadata: Some(UsageMetadata {
                prompt_token_count: prompt,
                candidates_token_count: candidates,
                total_token_count: prompt + candidates,
                cache_read_input_token_count: cache_read,
                cache_creation_input_token_count: cache_creation,
                ..Default::default()
            }),
            ..Default::default()
        };
        event
    }

    fn event_without_usage() -> Event {
        Event::new("test-invocation")
    }

    #[test]
    fn test_analyze_empty_events() {
        let metrics = CachePerformanceAnalyzer::analyze(&[]);
        assert_eq!(metrics.total_requests, 0);
        assert_eq!(metrics.requests_with_cache_hits, 0);
        assert_eq!(metrics.cache_hit_ratio, 0.0);
        assert_eq!(metrics.cache_utilization_ratio, 0.0);
    }

    #[test]
    fn test_analyze_single_event_no_cache() {
        let events = vec![event_with_usage(1000, 200, None, None)];
        let metrics = CachePerformanceAnalyzer::analyze(&events);
        assert_eq!(metrics.total_requests, 1);
        assert_eq!(metrics.requests_with_cache_hits, 0);
        assert_eq!(metrics.total_prompt_tokens, 1000);
        assert_eq!(metrics.total_cache_read_tokens, 0);
        assert_eq!(metrics.total_cache_creation_tokens, 0);
        assert_eq!(metrics.cache_hit_ratio, 0.0);
        assert_eq!(metrics.cache_utilization_ratio, 0.0);
        assert_eq!(metrics.avg_cached_tokens_per_request, 0.0);
    }

    #[test]
    fn test_analyze_single_event_with_cache_hit() {
        let events = vec![event_with_usage(1000, 200, Some(500), None)];
        let metrics = CachePerformanceAnalyzer::analyze(&events);
        assert_eq!(metrics.total_requests, 1);
        assert_eq!(metrics.requests_with_cache_hits, 1);
        assert_eq!(metrics.total_prompt_tokens, 1000);
        assert_eq!(metrics.total_cache_read_tokens, 500);
        assert_eq!(metrics.cache_hit_ratio, 50.0);
        assert_eq!(metrics.cache_utilization_ratio, 100.0);
        assert_eq!(metrics.avg_cached_tokens_per_request, 500.0);
    }

    #[test]
    fn test_analyze_mixed_events() {
        let events = vec![
            event_with_usage(1000, 200, Some(800), Some(200)),
            event_with_usage(1000, 300, None, None),
            event_with_usage(1000, 100, Some(600), None),
            event_without_usage(), // skipped
        ];
        let metrics = CachePerformanceAnalyzer::analyze(&events);
        assert_eq!(metrics.total_requests, 3);
        assert_eq!(metrics.requests_with_cache_hits, 2);
        assert_eq!(metrics.total_prompt_tokens, 3000);
        assert_eq!(metrics.total_cache_read_tokens, 1400);
        assert_eq!(metrics.total_cache_creation_tokens, 200);
        // cache_hit_ratio = 1400 / 3000 * 100 ≈ 46.67
        assert!((metrics.cache_hit_ratio - 46.666_666_666_666_664).abs() < 1e-10);
        // cache_utilization_ratio = 2 / 3 * 100 ≈ 66.67
        assert!((metrics.cache_utilization_ratio - 66.666_666_666_666_66).abs() < 1e-10);
        // avg_cached_tokens_per_request = 1400 / 3 ≈ 466.67
        assert!((metrics.avg_cached_tokens_per_request - 466.666_666_666_666_7).abs() < 1e-10);
    }

    #[test]
    fn test_analyze_all_cache_hits() {
        let events = vec![
            event_with_usage(500, 100, Some(500), None),
            event_with_usage(500, 100, Some(500), None),
        ];
        let metrics = CachePerformanceAnalyzer::analyze(&events);
        assert_eq!(metrics.total_requests, 2);
        assert_eq!(metrics.requests_with_cache_hits, 2);
        assert_eq!(metrics.cache_hit_ratio, 100.0);
        assert_eq!(metrics.cache_utilization_ratio, 100.0);
        assert_eq!(metrics.avg_cached_tokens_per_request, 500.0);
    }

    #[test]
    fn test_analyze_zero_prompt_tokens() {
        // Edge case: usage_metadata present but prompt_token_count is 0
        let events = vec![event_with_usage(0, 100, None, None)];
        let metrics = CachePerformanceAnalyzer::analyze(&events);
        assert_eq!(metrics.total_requests, 1);
        assert_eq!(metrics.total_prompt_tokens, 0);
        // cache_hit_ratio stays 0.0 (no division by zero)
        assert_eq!(metrics.cache_hit_ratio, 0.0);
        assert_eq!(metrics.cache_utilization_ratio, 0.0);
    }

    #[test]
    fn test_analyze_cache_creation_only() {
        let events = vec![event_with_usage(2000, 500, None, Some(1500))];
        let metrics = CachePerformanceAnalyzer::analyze(&events);
        assert_eq!(metrics.total_requests, 1);
        assert_eq!(metrics.requests_with_cache_hits, 0);
        assert_eq!(metrics.total_cache_creation_tokens, 1500);
        assert_eq!(metrics.cache_hit_ratio, 0.0);
        assert_eq!(metrics.cache_utilization_ratio, 0.0);
    }
}
