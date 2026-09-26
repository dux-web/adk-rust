use super::*;
use adk_core::ErrorCategory;
use adk_model::openai::OpenAIReasoningEffort;
use proptest::prelude::*;

#[test]
fn requires_explicit_api_for_unknown_models() {
    let error = OpenCodeGoClient::new(config("future-model")).err().unwrap();
    assert_eq!(error.code, "model.opencode_go.invalid_config");
    assert!(
        OpenCodeGoClient::new(config("future-model").with_api(OpenCodeGoApi::Messages)).is_ok()
    );
}

#[test]
fn validates_identity_endpoint_and_reasoning_without_requests() {
    for config in [
        OpenCodeGoConfig::new("secret", "minimax-m3"),
        config("minimax-m3").with_session_id("\ninvalid"),
        config("minimax-m3").with_user_agent(""),
        config("minimax-m3").with_reasoning_effort(OpenAIReasoningEffort::High),
        config("deepseek-v4.1-flash").with_anthropic_effort(adk_model::anthropic::Effort::High),
        config("deepseek-v4.1-flash").with_base_url("http://example.com/v1"),
        config("deepseek-v4.1-flash").with_base_url("https://opencode.ai/zen/go/v1?secret=secret"),
    ] {
        let error = OpenCodeGoClient::new(config).err().unwrap();
        assert_eq!(error.category, ErrorCategory::InvalidInput);
        assert!(!error.to_string().contains("secret"));
    }
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(100))]
    #[test]
    fn debug_omits_credentials(key in "[a-zA-Z0-9]{30,60}") {
        let config = OpenCodeGoConfig::new(&key, "minimax-m3");
        let debug = format!("{config:?}");
        prop_assert!(!debug.contains(&key));
    }
}
