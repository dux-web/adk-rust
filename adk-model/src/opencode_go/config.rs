use crate::anthropic::{Effort, ThinkingMode};
use crate::openai::OpenAIReasoningEffort;
use crate::retry::RetryConfig;

/// Wire API accepted by an OpenCode Go model.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OpenCodeGoApi {
    /// OpenAI-compatible Chat Completions.
    ChatCompletions,
    /// OpenAI Responses.
    Responses,
    /// Anthropic Messages.
    Messages,
}

impl OpenCodeGoApi {
    /// Returns the API in the published Go endpoint table, or `None` for an unknown model.
    ///
    /// Unknown models require an explicit selection with [`OpenCodeGoConfig::with_api`].
    /// Source: <https://opencode.ai/docs/go/#endpoints>.
    pub fn for_model(model: &str) -> Option<Self> {
        match model {
            "grok-4.7"
            | "grok-4.6"
            | "gpt-6-luna"
            | "gpt-5.6-luna"
            | "muse-spark-1.3-contributor"
            | "muse-spark-1.2-contributor" => Some(Self::Responses),
            "minimax-m3" | "minimax-m2.7" | "minimax-m2.5" | "qwen3.8-max" | "qwen3.8-flash"
            | "qwen3.7-max" | "qwen3.7-plus" | "qwen3.6-plus" => Some(Self::Messages),
            "glm-5.3-flash"
            | "glm-5.3"
            | "glm-5.2"
            | "glm-5.1"
            | "kimi-k3"
            | "kimi-k2.7-code"
            | "kimi-k2.6"
            | "longcat-2.0"
            | "deepseek-v4.1-flash"
            | "deepseek-v4-pro"
            | "deepseek-v4-flash"
            | "deepseek-v4-flash-vision-exp"
            | "mimo-v2.6-flash"
            | "mimo-v2.6-pro"
            | "mimo-v2.5"
            | "mimo-v2.5-pro"
            | "hy4-preview"
            | "hy3"
            | "space-bunny-free"
            | "longcat-2.5-preview-free" => Some(Self::ChatCompletions),
            _ => None,
        }
    }
}

/// Configuration for one OpenCode Go model and conversation.
///
/// Set the application's own user agent and reuse the session ID for all main
/// and auxiliary requests in the same conversation. Credentials are omitted
/// from the debug representation.
///
/// # Example
///
/// ```
/// use adk_model::opencode_go::OpenCodeGoConfig;
/// let config = OpenCodeGoConfig::new("api-key", "deepseek-v4.1-flash")
///     .with_user_agent("example-coding-agent/1.0")
///     .with_session_id("conversation-42");
/// ```
#[derive(Clone)]
pub struct OpenCodeGoConfig {
    pub(super) api_key: String,
    pub(super) model: String,
    pub(super) base_url: String,
    pub(super) api: Option<OpenCodeGoApi>,
    pub(super) user_agent: String,
    pub(super) session_id: String,
    pub(super) reasoning_effort: Option<OpenAIReasoningEffort>,
    pub(super) thinking: Option<ThinkingMode>,
    pub(super) anthropic_effort: Option<Effort>,
    pub(super) retry: RetryConfig,
}

impl std::fmt::Debug for OpenCodeGoConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OpenCodeGoConfig")
            .field("model", &self.model)
            .field("api", &self.api)
            .finish_non_exhaustive()
    }
}

impl OpenCodeGoConfig {
    /// Creates a config with the official Go base URL and automatic API selection.
    pub fn new(api_key: impl Into<String>, model: impl Into<String>) -> Self {
        Self {
            api_key: api_key.into(),
            model: model.into(),
            base_url: "https://opencode.ai/zen/go/v1".into(),
            api: None,
            user_agent: String::new(),
            session_id: String::new(),
            reasoning_effort: None,
            thinking: None,
            anthropic_effort: None,
            retry: RetryConfig::default(),
        }
    }

    /// Sets the API explicitly, including for models added after this library release.
    pub fn with_api(mut self, api: OpenCodeGoApi) -> Self {
        self.api = Some(api);
        self
    }

    /// Sets a proxy or test endpoint including its `/v1` suffix.
    pub fn with_base_url(mut self, base_url: impl Into<String>) -> Self {
        self.base_url = base_url.into();
        self
    }

    /// Identifies the calling coding application, such as `my-coding-agent/1.0`.
    pub fn with_user_agent(mut self, user_agent: impl Into<String>) -> Self {
        self.user_agent = user_agent.into();
        self
    }

    /// Sets the stable conversation ID sent as `x-opencode-session`.
    pub fn with_session_id(mut self, session_id: impl Into<String>) -> Self {
        self.session_id = session_id.into();
        self
    }

    /// Sets reasoning effort for Chat Completions or Responses models.
    ///
    /// Messages models use [`Self::with_anthropic_effort`] instead.
    pub fn with_reasoning_effort(mut self, effort: OpenAIReasoningEffort) -> Self {
        self.reasoning_effort = Some(effort);
        self
    }

    /// Sets thinking for models using Anthropic Messages.
    pub fn with_anthropic_thinking(mut self, thinking: ThinkingMode) -> Self {
        self.thinking = Some(thinking);
        self
    }

    /// Sets output effort for models using Anthropic Messages.
    pub fn with_anthropic_effort(mut self, effort: Effort) -> Self {
        self.anthropic_effort = Some(effort);
        self
    }

    /// Sets the underlying model client's retry policy.
    pub fn with_retry_config(mut self, retry: RetryConfig) -> Self {
        self.retry = retry;
        self
    }
}
