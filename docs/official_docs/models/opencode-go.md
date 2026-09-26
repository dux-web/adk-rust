# OpenCode Go

Enable `adk-model`'s `opencode-go` feature, or the same feature on `adk-rust`.

```rust
use adk_model::opencode_go::{OpenCodeGoClient, OpenCodeGoConfig};

let model = OpenCodeGoClient::new(
    OpenCodeGoConfig::new(std::env::var("OPENCODE_API_KEY")?, "deepseek-v4.1-flash")
        .with_user_agent("my-coding-agent/1.0")
        .with_session_id("conversation-42"),
)?;
# Ok::<(), Box<dyn std::error::Error>>(())
```

`OpenCodeGoClient` implements `adk_core::Llm` and delegates to the existing protocol clients. The model's entry in the [Go endpoint table](https://opencode.ai/docs/go/#endpoints) selects Chat Completions, Responses, or Anthropic Messages. The base URL is `https://opencode.ai/zen/go/v1`.

- Supply the application's own `User-Agent` and a stable `x-opencode-session` value. Reuse the ID for main and auxiliary requests in the same conversation.
- Unknown model IDs require `with_api(OpenCodeGoApi::...)`; the client does not guess a protocol or retry on another API.
- Chat Completions replays returned thinking through `reasoning_content` for tool continuations.
- Chat Completions and Responses accept `with_reasoning_effort`. Messages uses `with_anthropic_thinking` and `with_anthropic_effort`. Choose options supported by the selected model.
- `with_base_url` supports HTTPS proxies and loopback HTTP tests. Include the `/v1` suffix.
- Use `LlmRequest.config` for sampling and output limits. `with_retry_config` configures the selected model client's retry policy.

The standalone `examples/opencode_go` crate demonstrates streaming. Offline HTTP tests cover API routing, identity headers, usage conversion, tool continuations, and invalid configuration; they do not establish live account availability or quota.
