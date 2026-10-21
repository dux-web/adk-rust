//! Provenance-bound provider history projection.
use crate::{Content, Part, ProviderData, ProviderOrigin};

/// Adds provider-owned replay parts to a copy of an assistant's content.
///
/// Providers can keep opaque continuation items in `conversation_history_parts`,
/// either at the metadata root or under `openai`. These parts belong in model
/// history alongside the visible answer, even when no reasoning summary exists.
/// Already present parts are retained once, in their original order.
///
/// When metadata carries a valid `provider_origin`, opaque parts are enclosed in
/// [`ProviderData`] for an ownership-checked [`for_provider`] projection.
/// Without one, parts stay ordinary and replay to the next request unchanged,
/// as the same model expects within an invocation (signed thinking, encrypted
/// reasoning and thought signatures). An application that switches providers or
/// accounts records an origin and projects with [`for_provider`].
/// The source content and metadata remain unchanged.
///
/// # Example
///
/// ```rust
/// use adk_core::{Content, Part};
/// use adk_core::history::with_provider_parts;
/// use serde_json::json;
///
/// let content = Content::new("model").with_text("Done");
/// let metadata = json!({"openai":{"conversation_history_parts":[{
///     "server_tool_call":{
///         "type":"reasoning", "id":"rs_1", "summary":[],
///         "encrypted_content":"opaque"
///     }
/// }]}});
/// let history = with_provider_parts(&content, Some(&metadata));
/// assert_eq!(content.parts.len(), 1);
/// assert_eq!(history.parts.len(), 2);
/// assert!(matches!(history.parts[1], Part::ServerToolCall { .. }));
///
/// let mut owned = metadata.clone();
/// owned["provider_origin"] = json!({"provider":"openai","model":"gpt","scope":"account"});
/// let history = with_provider_parts(&content, Some(&owned));
/// assert!(matches!(history.parts[1], Part::ProviderData { .. }));
/// ```
pub fn with_provider_parts(content: &Content, metadata: Option<&serde_json::Value>) -> Content {
    let mut content = content.clone();
    let parts = metadata
        .and_then(|metadata| {
            metadata.get("conversation_history_parts").or_else(|| {
                metadata.get("openai").and_then(|openai| openai.get("conversation_history_parts"))
            })
        })
        .and_then(serde_json::Value::as_array);
    for part in parts.into_iter().flatten() {
        if let Ok(part) = serde_json::from_value::<Part>(part.clone())
            && !content.parts.iter().any(|existing| {
                existing == &part
                    || matches!(existing, Part::ProviderData { provider_data } if provider_data.parts.contains(&part))
            })
        {
            content.parts.push(part);
        }
    }
    let Some(origin) = provider_origin(metadata) else {
        return content;
    };
    content.parts = content
        .parts
        .into_iter()
        .map(|part| {
            if opaque(&part)
                || matches!(&part, Part::FunctionCall { thought_signature: Some(_), .. })
            {
                Part::ProviderData {
                    provider_data: ProviderData { origin: Some(origin.clone()), parts: vec![part] },
                }
            } else {
                part
            }
        })
        .collect();
    content
}

/// Reads a valid `provider_origin` from response metadata.
///
/// Missing, malformed or incomplete ownership returns `None`.
pub fn provider_origin(metadata: Option<&serde_json::Value>) -> Option<ProviderOrigin> {
    metadata
        .and_then(|metadata| metadata.get("provider_origin"))
        .and_then(|origin| serde_json::from_value::<ProviderOrigin>(origin.clone()).ok())
        .filter(valid_origin)
}

fn valid_origin(origin: &ProviderOrigin) -> bool {
    !origin.provider.is_empty() && !origin.model.is_empty() && !origin.scope.is_empty()
}

fn opaque(part: &Part) -> bool {
    matches!(
        part,
        Part::Thinking { .. } | Part::ServerToolCall { .. } | Part::ServerToolResponse { .. }
    )
}

/// Projects history for one proven provider, model and authorization owner.
///
/// Matching [`ProviderData`] is expanded in place. Foreign and originless opaque
/// parts are omitted, including old records that predate ownership metadata.
/// Normal text, function calls and tool results are retained; a function call's
/// provider thought signature is removed unless its envelope proves its owner.
/// The internal envelope is never part of the resulting provider request.
///
/// This only changes the request's copy of history. Persisted events and their
/// original provider payloads remain unchanged.
///
/// ```rust
/// use adk_core::{Content, Part, ProviderData, ProviderOrigin};
/// use adk_core::history::for_provider;
/// let origin = ProviderOrigin { provider: "provider".into(), model: "model".into(), scope: "owner".into() };
/// let mut history = vec![Content::new("model").with_thinking("unowned").with_text("Answer")];
/// for_provider(&mut history, &origin);
/// assert_eq!(history[0].parts, vec![Part::Text { text: "Answer".into() }]);
/// ```
pub fn for_provider(contents: &mut Vec<Content>, origin: &ProviderOrigin) {
    for content in contents.iter_mut() {
        let mut parts = Vec::with_capacity(content.parts.len());
        for part in std::mem::take(&mut content.parts) {
            match part {
                Part::ProviderData { provider_data } => {
                    let owned =
                        valid_origin(origin) && provider_data.origin.as_ref() == Some(origin);
                    for mut part in provider_data.parts {
                        if opaque(&part) {
                            if owned {
                                parts.push(part);
                            }
                        } else if !matches!(part, Part::ProviderData { .. }) {
                            if !owned
                                && let Part::FunctionCall { thought_signature, .. } = &mut part
                            {
                                *thought_signature = None;
                            }
                            parts.push(part);
                        }
                    }
                }
                mut part => {
                    if opaque(&part) {
                        continue;
                    }
                    if let Part::FunctionCall { thought_signature, .. } = &mut part {
                        *thought_signature = None;
                    }
                    parts.push(part);
                }
            }
        }
        content.parts = parts;
    }
    contents.retain(|content| !content.parts.is_empty());
}

#[cfg(test)]
mod tests;
