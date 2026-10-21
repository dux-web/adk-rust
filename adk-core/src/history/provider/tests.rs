use super::*;
use serde_json::json;

fn origin() -> ProviderOrigin {
    ProviderOrigin {
        provider: "responses".into(),
        model: "reasoning-model".into(),
        scope: "owner".into(),
    }
}

fn call(signature: Option<&str>) -> Part {
    Part::FunctionCall {
        name: "lookup".into(),
        args: json!({"key":"value"}),
        id: Some("call_1".into()),
        thought_signature: signature.map(str::to_owned),
    }
}

fn content() -> Content {
    Content {
        role: "model".into(),
        parts: vec![
            Part::Thinking {
                thinking: "Private thought".into(),
                signature: Some("private-signature".into()),
            },
            Part::Text { text: "Visible answer".into() },
            call(Some("call-signature")),
        ],
    }
}

fn metadata() -> serde_json::Value {
    json!({"provider_origin": origin(), "openai":{"conversation_history_parts":[{
        "server_tool_call":{"type":"reasoning","id":"rs_1","summary":[],"encrypted_content":"opaque"}
    }]}})
}

#[test]
fn preserves_source_order_and_deduplicates() {
    let original = content();
    let metadata = metadata();
    let bytes = serde_json::to_vec(&original).unwrap();
    let history = with_provider_parts(&original, Some(&metadata));
    assert!(matches!(history.parts[0], Part::ProviderData { .. }));
    assert_eq!(history.parts[0].provider_part(), &original.parts[0]);
    assert_eq!(history.parts[1], original.parts[1]);
    assert_eq!(history.parts[2].provider_part(), &original.parts[2]);
    assert!(history.has_function_calls());
    let twice = with_provider_parts(&history, Some(&metadata));
    assert_eq!(twice.parts, history.parts);
    assert_eq!(serde_json::to_vec(&original).unwrap(), bytes);
    let mut sent = vec![history];
    for_provider(&mut sent, &origin());
    assert_eq!(&sent[0].parts[..3], &original.parts);
    assert!(matches!(sent[0].parts[3], Part::ServerToolCall { .. }));
    assert!(!serde_json::to_string(&sent).unwrap().contains("provider_data"));
}

#[test]
fn isolates_each_ownership_component() {
    for field in ["provider", "model", "scope"] {
        let mut target = origin();
        match field {
            "provider" => target.provider = "gemini".into(),
            "model" => target.model = "other-model".into(),
            "scope" => target.scope = "other-account-or-endpoint".into(),
            _ => unreachable!(),
        }
        let projected = with_provider_parts(&content(), Some(&metadata()));
        let bytes = serde_json::to_vec(&projected).unwrap();
        let mut sent = vec![projected.clone()];
        for_provider(&mut sent, &target);
        assert_eq!(
            sent[0].parts,
            vec![Part::Text { text: "Visible answer".into() }, call(None)],
            "{field}"
        );
        assert_eq!(serde_json::to_vec(&projected).unwrap(), bytes);
    }
}

#[test]
fn treats_old_records_as_unowned() {
    let old = with_provider_parts(&content(), Some(&json!({"openai": metadata()["openai"]})));
    let mut sent = vec![old.clone(), Content::new("model").with_thinking("Old thought")];
    for_provider(&mut sent, &origin());
    assert_eq!(sent.len(), 1);
    assert_eq!(sent[0].parts, vec![Part::Text { text: "Visible answer".into() }, call(None)]);
    assert!(!old.parts.iter().any(|part| matches!(part, Part::ProviderData { .. })));
}

#[test]
fn keeps_tool_results_and_rejects_nested_data() {
    let response = Part::FunctionResponse {
        function_response: crate::FunctionResponseData::new("lookup", json!({"value":42})),
        id: Some("call_1".into()),
        annotations: None,
    };
    let nested = Part::ProviderData {
        provider_data: ProviderData {
            origin: Some(origin()),
            parts: vec![
                Part::ProviderData {
                    provider_data: ProviderData { origin: Some(origin()), parts: content().parts },
                },
                response.clone(),
            ],
        },
    };
    let mut sent = vec![Content { role: "tool".into(), parts: vec![nested] }];
    for_provider(&mut sent, &origin());
    assert_eq!(sent[0].parts, vec![response]);
}

#[test]
fn tracks_generated_ids_inside_envelopes() {
    let mut source = content();
    if let Part::FunctionCall { id, .. } = &mut source.parts[2] {
        *id = None;
    }
    let mut projected = with_provider_parts(&source, Some(&metadata()));
    projected.assign_call_ids();
    assert!(matches!(projected.parts[2].provider_part(), Part::FunctionCall { id: Some(_), .. }));
    projected.strip_generated_call_ids();
    assert!(matches!(projected.parts[2].provider_part(), Part::FunctionCall { id: None, .. }));
}

#[test]
fn owned_data_requires_projection() {
    let projected = with_provider_parts(&content(), Some(&metadata()));
    let model = projected.for_model();
    assert_eq!(model.parts, vec![Part::Text { text: "Visible answer".into() }, call(None)]);
    assert_eq!(projected.parts[0].provider_part(), &content().parts[0]);
}

#[test]
fn originless_parts_replay_unchanged() {
    let replay = Part::ServerToolCall {
        server_tool_call: json!({"type":"reasoning","id":"rs_1","summary":[],"encrypted_content":"opaque"}),
    };
    for metadata in [
        None,
        Some(json!({"openai": metadata()["openai"]})),
        Some(json!({"provider_origin":{"provider":"","model":"m","scope":"s"},
            "openai": metadata()["openai"]})),
    ] {
        let projected = with_provider_parts(&content(), metadata.as_ref());
        let mut expected = content().parts;
        if metadata.is_some() {
            expected.push(replay.clone());
        }
        assert_eq!(projected.parts, expected);
        assert_eq!(projected.for_model().parts, expected);
    }
}
