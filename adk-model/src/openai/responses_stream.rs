//! Responses stream text recovery, separate from transport and request policy.

use std::collections::{BTreeMap, BTreeSet, HashSet};

use adk_core::{Content, LlmResponse, Part};
use async_openai::types::responses::{
    MessagePhase, OutputContent, OutputItem, OutputMessageContent, ReasoningItemContent,
    ResponseStreamEvent, SummaryPart,
};

use super::responses_convert;
#[cfg(test)]
use async_openai::types::responses::Response;

#[derive(Clone, Copy, Eq, Ord, PartialEq, PartialOrd)]
enum TextKind {
    Answer,
    Refusal,
    Summary,
    Reasoning,
}

#[derive(Clone, Eq, Ord, PartialEq, PartialOrd)]
struct TextKey {
    output_index: u32,
    part_index: u32,
    item_id: String,
    kind: TextKind,
}

impl TextKey {
    fn new(item_id: &str, output_index: u32, part_index: u32, kind: TextKind) -> Self {
        Self { item_id: item_id.into(), output_index, part_index, kind }
    }
}

#[derive(Default)]
pub(super) struct StreamState {
    emitted: BTreeMap<TextKey, String>,
    finalized: BTreeMap<TextKey, String>,
    finalized_items: BTreeMap<u32, OutputItem>,
    pending_calls: BTreeMap<u32, Option<String>>,
    commentary: HashSet<(u32, String)>,
    native_tools: bool,
    corrected: bool,
}

impl StreamState {
    pub(super) fn new(native_tools: bool) -> Self {
        Self { native_tools, ..Default::default() }
    }

    pub(super) fn partial(&mut self, event: &ResponseStreamEvent) -> Option<LlmResponse> {
        use ResponseStreamEvent::*;

        let mut parts = Vec::new();
        match event {
            ResponseOutputItemAdded(event) => {
                match &event.item {
                    OutputItem::FunctionCall(call) => {
                        self.pending_calls.insert(event.output_index, Some(call.call_id.clone()));
                    }
                    OutputItem::CustomToolCall(call) => {
                        self.pending_calls.insert(event.output_index, Some(call.call_id.clone()));
                    }
                    _ => {}
                }
                self.record_phase(event.output_index, &event.item);
                return None;
            }
            ResponseFunctionCallArgumentsDelta(event) => {
                self.pending_calls.entry(event.output_index).or_default();
                return None;
            }
            ResponseCustomToolCallInputDelta(event) => {
                self.pending_calls.entry(event.output_index).or_default();
                return None;
            }
            ResponseOutputTextDelta(event) => {
                let key = TextKey::new(
                    &event.item_id,
                    event.output_index,
                    event.content_index,
                    TextKind::Answer,
                );
                self.delta(key, &event.delta, &mut parts);
            }
            ResponseOutputTextDone(event) => {
                let key = TextKey::new(
                    &event.item_id,
                    event.output_index,
                    event.content_index,
                    TextKind::Answer,
                );
                self.done(key, &event.text, &mut parts);
            }
            ResponseRefusalDelta(event) => {
                let key = TextKey::new(
                    &event.item_id,
                    event.output_index,
                    event.content_index,
                    TextKind::Refusal,
                );
                self.delta(key, &event.delta, &mut parts);
            }
            ResponseRefusalDone(event) => {
                let key = TextKey::new(
                    &event.item_id,
                    event.output_index,
                    event.content_index,
                    TextKind::Refusal,
                );
                self.done(key, &event.refusal, &mut parts);
            }
            ResponseContentPartDone(event) => match &event.part {
                OutputContent::OutputText(text) => {
                    let key = TextKey::new(
                        &event.item_id,
                        event.output_index,
                        event.content_index,
                        TextKind::Answer,
                    );
                    self.done(key, &text.text, &mut parts);
                }
                OutputContent::Refusal(refusal) => {
                    let key = TextKey::new(
                        &event.item_id,
                        event.output_index,
                        event.content_index,
                        TextKind::Refusal,
                    );
                    self.done(key, &refusal.refusal, &mut parts);
                }
                OutputContent::ReasoningText(_) => return None,
            },
            ResponseReasoningSummaryTextDelta(event) => {
                let key = TextKey::new(
                    &event.item_id,
                    event.output_index,
                    event.summary_index,
                    TextKind::Summary,
                );
                self.delta(key, &event.delta, &mut parts);
            }
            ResponseReasoningSummaryTextDone(event) => {
                let key = TextKey::new(
                    &event.item_id,
                    event.output_index,
                    event.summary_index,
                    TextKind::Summary,
                );
                self.done(key, &event.text, &mut parts);
            }
            ResponseReasoningSummaryPartDone(event) => {
                let SummaryPart::SummaryText(text) = &event.part;
                let key = TextKey::new(
                    &event.item_id,
                    event.output_index,
                    event.summary_index,
                    TextKind::Summary,
                );
                self.done(key, &text.text, &mut parts);
            }
            ResponseReasoningTextDelta(event) => {
                let key = TextKey::new(
                    &event.item_id,
                    event.output_index,
                    event.content_index,
                    TextKind::Reasoning,
                );
                self.delta(key, &event.delta, &mut parts);
            }
            ResponseReasoningTextDone(event) => {
                let key = TextKey::new(
                    &event.item_id,
                    event.output_index,
                    event.content_index,
                    TextKind::Reasoning,
                );
                self.done(key, &event.text, &mut parts);
            }
            ResponseOutputItemDone(event) => {
                // A complete done call may replace its whole identity, never mix
                // earlier fragments into that authoritative payload.
                if let Some(pending) = self.pending_calls.get_mut(&event.output_index)
                    && let Some(call_id) = super::wire::finalized_call(&event.item)
                {
                    *pending = Some(call_id.to_owned());
                }
                self.finalized_items.insert(event.output_index, event.item.clone());
                self.record_phase(event.output_index, &event.item);
                let mut item = event.item.clone();
                self.trim_item(event.output_index, &mut item, &mut parts);
            }
            _ => return None,
        }
        Some(LlmResponse {
            content: (!parts.is_empty()).then_some(Content { role: "model".into(), parts }),
            partial: true,
            ..Default::default()
        })
    }

    #[cfg(test)]
    pub(super) fn complete(&mut self, response: &Response) -> LlmResponse {
        self.finish(&super::wire::Response::from(response)).unwrap()
    }

    pub(super) fn finish(
        &mut self,
        response: &super::wire::Response,
    ) -> Result<LlmResponse, adk_core::AdkError> {
        if response.output_missing && self.finalized_items.is_empty() {
            return Err(super::wire::missing_output());
        }
        let mut response = response.clone();
        let recovered = response.output.is_empty() && !self.finalized_items.is_empty();
        if recovered {
            // Codex can omit terminal output after emitting authoritative done items.
            // Recover the whole items, including calls and reasoning, not only text.
            response.output = self.finalized_items.values().cloned().collect();
        }
        super::wire::validate(&response)?;
        for (index, expected) in &self.pending_calls {
            fn call_id(item: &OutputItem) -> Option<&str> {
                match item {
                    OutputItem::FunctionCall(call) => Some(call.call_id.as_str()),
                    OutputItem::CustomToolCall(call) => Some(call.call_id.as_str()),
                    _ => None,
                }
            }
            let identity = if let Some(expected) = expected {
                response.output.iter().filter_map(call_id).find(|identity| *identity == expected)
            } else {
                response.output.get(*index as usize).and_then(call_id)
            };
            if identity.is_none()
                || expected.as_deref().zip(identity).is_some_and(|(left, right)| left != right)
            {
                return Err(super::wire::unfinished());
            }
        }
        let mut full = responses_convert::from_projection(&response);
        let mut unseen = response.clone();
        let mut final_keys = BTreeSet::new();
        for (output_index, item) in unseen.output.iter_mut().enumerate() {
            final_keys.extend(Self::item_keys(output_index as u32, item));
            self.record_phase(output_index as u32, item);
            self.trim_item(output_index as u32, item, &mut Vec::new());
        }
        let omitted = response.output.is_empty();
        let removed = !omitted && self.emitted.keys().any(|key| !final_keys.contains(key));
        let unfinished = self.emitted.keys().any(|key| !self.finalized.contains_key(key));
        if self.corrected
            || recovered
            || removed
            || unfinished
            || self.native_tools
            || !self.commentary.is_empty()
        {
            // ADK replaces accumulated fragments with this authoritative snapshot.
            if omitted {
                full.content = Some(Content { role: "model".into(), parts: self.snapshot_parts() });
            } else if (self.corrected || removed || unfinished) && full.content.is_none() {
                full.content = Some(Content { role: "model".into(), parts: Vec::new() });
            }
            full.provider_metadata.get_or_insert_with(|| serde_json::json!({}))["content_complete"] =
                serde_json::json!(true);
        } else {
            // Keep metadata/citation offsets from the full response, not its suffixes.
            full.content = responses_convert::from_projection(&unseen).content;
        }
        Ok(full)
    }

    fn record_phase(&mut self, output_index: u32, item: &OutputItem) {
        if let OutputItem::Message(message) = item
            && message.phase == Some(MessagePhase::Commentary)
        {
            self.commentary.insert((output_index, message.id.clone()));
        }
    }

    fn delta(&mut self, key: TextKey, text: &str, parts: &mut Vec<Part>) {
        if self.emitted.iter().any(|(previous, text)| previous > &key && !text.is_empty()) {
            self.corrected = true;
        }
        self.emitted.entry(key.clone()).or_default().push_str(text);
        if let Some(part) = self.part(&key, text, "in_progress") {
            parts.push(part);
        }
    }

    fn done(&mut self, key: TextKey, text: &str, parts: &mut Vec<Part>) -> String {
        self.finalized.insert(key.clone(), text.into());
        let later_text =
            self.emitted.iter().any(|(previous, text)| previous > &key && !text.is_empty());
        let emitted = self.emitted.entry(key.clone()).or_default();
        let Some(suffix) = text.strip_prefix(emitted.as_str()) else {
            // A final payload can correct earlier text, but it is not another delta.
            self.corrected = true;
            return String::new();
        };
        let suffix = suffix.to_owned();
        if later_text && !suffix.is_empty() {
            // Inserting an earlier part requires replacement, not an appended suffix.
            self.corrected = true;
            return String::new();
        }
        emitted.push_str(&suffix);
        if let Some(part) = self.part(&key, &suffix, "in_progress") {
            parts.push(part);
        }
        suffix
    }

    fn snapshot_parts(&self) -> Vec<Part> {
        // An omitted output retains done payloads, not unfinished streamed deltas.
        let mut parts: Vec<Part> = Vec::new();
        let mut messages: BTreeMap<(u32, String), usize> = BTreeMap::new();
        for (key, text) in &self.finalized {
            let Some(part) = self.part(key, text, "completed") else {
                continue;
            };
            if let Part::ServerToolCall { server_tool_call } = &part {
                let identity = (key.output_index, key.item_id.clone());
                if let Some(index) = messages.get(&identity) {
                    let Part::ServerToolCall { server_tool_call: message } = &mut parts[*index]
                    else {
                        unreachable!("commentary messages use native parts");
                    };
                    message["content"]
                        .as_array_mut()
                        .expect("native message content is an array")
                        .extend(
                            server_tool_call["content"]
                                .as_array()
                                .expect("native message content is an array")
                                .iter()
                                .cloned(),
                        );
                    continue;
                }
                messages.insert(identity, parts.len());
            }
            parts.push(part);
        }
        parts
    }

    fn item_keys(output_index: u32, item: &OutputItem) -> Vec<TextKey> {
        match item {
            OutputItem::Message(message) => message
                .content
                .iter()
                .enumerate()
                .map(|(index, part)| {
                    let kind = match part {
                        OutputMessageContent::OutputText(_) => TextKind::Answer,
                        OutputMessageContent::Refusal(_) => TextKind::Refusal,
                    };
                    TextKey::new(&message.id, output_index, index as u32, kind)
                })
                .collect(),
            OutputItem::Reasoning(reasoning) if !reasoning.summary.is_empty() => reasoning
                .summary
                .iter()
                .enumerate()
                .map(|(index, _)| {
                    TextKey::new(
                        reasoning.id.as_deref().unwrap_or_default(),
                        output_index,
                        index as u32,
                        TextKind::Summary,
                    )
                })
                .collect(),
            OutputItem::Reasoning(reasoning) => reasoning
                .content
                .as_deref()
                .unwrap_or_default()
                .iter()
                .enumerate()
                .map(|(index, _)| {
                    TextKey::new(
                        reasoning.id.as_deref().unwrap_or_default(),
                        output_index,
                        index as u32,
                        TextKind::Reasoning,
                    )
                })
                .collect(),
            _ => Vec::new(),
        }
    }

    fn trim_item(&mut self, output_index: u32, item: &mut OutputItem, parts: &mut Vec<Part>) {
        match item {
            OutputItem::Message(message) => {
                for (part_index, content) in message.content.iter_mut().enumerate() {
                    let (kind, text) = match content {
                        OutputMessageContent::OutputText(text) => {
                            (TextKind::Answer, &mut text.text)
                        }
                        OutputMessageContent::Refusal(refusal) => {
                            (TextKind::Refusal, &mut refusal.refusal)
                        }
                    };
                    let key = TextKey::new(&message.id, output_index, part_index as u32, kind);
                    *text = self.done(key, text, parts);
                }
            }
            OutputItem::Reasoning(reasoning) => {
                let item_id = reasoning.id.as_deref().unwrap_or_default();
                for (part_index, summary) in reasoning.summary.iter_mut().enumerate() {
                    let SummaryPart::SummaryText(text) = summary;
                    let key =
                        TextKey::new(item_id, output_index, part_index as u32, TextKind::Summary);
                    text.text = self.done(key, &text.text, parts);
                }
                if reasoning.summary.is_empty() {
                    for (part_index, content) in reasoning.content.iter_mut().flatten().enumerate()
                    {
                        let text = match content {
                            ReasoningItemContent::ReasoningText(text) => &mut text.text,
                        };
                        let key = TextKey::new(
                            item_id,
                            output_index,
                            part_index as u32,
                            TextKind::Reasoning,
                        );
                        *text = self.done(key, text, parts);
                    }
                }
            }
            _ => {}
        }
    }

    fn part(&self, key: &TextKey, text: &str, message_status: &str) -> Option<Part> {
        if text.is_empty() {
            return None;
        }
        if self.commentary.contains(&(key.output_index, key.item_id.clone())) {
            let content = if key.kind == TextKind::Refusal {
                serde_json::json!({"type": "refusal", "refusal": text})
            } else {
                serde_json::json!({"type": "output_text", "text": text, "annotations": []})
            };
            Some(Part::ServerToolCall {
                server_tool_call: serde_json::json!({
                    "type": "message", "role": "assistant", "id": key.item_id,
                    "phase": "commentary", "status": message_status, "content": [content]
                }),
            })
        } else if matches!(key.kind, TextKind::Summary | TextKind::Reasoning) {
            Some(Part::Thinking { thinking: text.into(), signature: None })
        } else {
            Some(Part::Text { text: text.into() })
        }
    }
}
