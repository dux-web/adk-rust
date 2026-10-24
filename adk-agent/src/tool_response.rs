//! Responses constructed by the tool dispatcher rather than returned by a tool.

use adk_core::{Content, FunctionResponseData, Part, TOOL_RESULT_STATUS_KEY, ToolResultStatus};
use serde_json::{Value, json};

pub(crate) struct ToolResponse {
    pub(crate) value: Value,
    status: Option<ToolResultStatus>,
}

impl ToolResponse {
    pub(crate) fn new(value: Value) -> Self {
        Self { value, status: None }
    }

    pub(crate) fn with_status(value: Value, status: ToolResultStatus) -> Self {
        Self { value, status: Some(status) }
    }

    pub(crate) fn failed(value: Value) -> Self {
        Self::with_status(value, ToolResultStatus::Failed)
    }

    pub(crate) fn error(message: impl Into<String>) -> Self {
        Self::failed(json!({ "error": message.into() }))
    }

    pub(crate) fn error_for(message: impl Into<String>, error: &adk_core::AdkError) -> Self {
        let mut response = Self::error(message);
        response.status = Some(Self::error_status(error));
        response
    }

    pub(crate) fn from_error(name: &str, error: &adk_core::AdkError, expose: bool) -> Self {
        Self::for_error(adk_core::tool_error_response(name, error, expose), error)
    }

    pub(crate) fn for_error(value: Value, error: &adk_core::AdkError) -> Self {
        Self::with_status(value, Self::error_status(error))
    }

    fn error_status(error: &adk_core::AdkError) -> ToolResultStatus {
        if error.category == adk_core::ErrorCategory::Cancelled {
            ToolResultStatus::Cancelled
        } else {
            ToolResultStatus::Failed
        }
    }

    pub(crate) fn into_part(self, name: &str, id: Option<&String>) -> Part {
        let mut part = Part::FunctionResponse {
            function_response: FunctionResponseData::from_tool_result(name, self.value),
            id: id.cloned(),
            annotations: None,
        };
        if let Some(status) = self.status {
            Self::annotate(&mut part, status);
        }
        part
    }

    pub(crate) fn into_content(self, name: &str, id: Option<&String>) -> Content {
        Content { role: "function".to_string(), parts: vec![self.into_part(name, id)] }
    }

    /// A result transform cannot establish whether an uncertain side effect completed.
    pub(crate) fn preserve_unknown(previous: &Content, content: &mut Content) {
        let Some(previous) = previous
            .parts
            .iter()
            .find(|part| part.tool_result_status() == Some(ToolResultStatus::OutcomeUnknown))
            .and_then(Part::annotations)
            .and_then(Value::as_object)
        else {
            return;
        };
        for part in &mut content.parts {
            let Part::FunctionResponse { annotations, .. } = part else { continue };
            let mut fields = previous.clone();
            if let Some(Value::Object(current)) = annotations.take() {
                fields.extend(current);
            }
            *annotations = Some(Value::Object(fields));
            Self::annotate(part, ToolResultStatus::OutcomeUnknown);
        }
    }

    pub(crate) fn annotate(part: &mut Part, status: ToolResultStatus) {
        if let Part::FunctionResponse { annotations, .. } = part {
            let mut fields =
                annotations.take().and_then(|value| value.as_object().cloned()).unwrap_or_default();
            fields.insert(TOOL_RESULT_STATUS_KEY.to_string(), json!(status));
            *annotations = Some(Value::Object(fields));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn replacement_keeps_unknown_annotations() {
        let mut previous = ToolResponse::with_status(
            json!({"status": "outcome_unknown"}),
            ToolResultStatus::OutcomeUnknown,
        )
        .into_content("charge", None);
        if let Part::FunctionResponse { annotations, .. } = &mut previous.parts[0] {
            *annotations = Some(json!({
                TOOL_RESULT_STATUS_KEY: ToolResultStatus::OutcomeUnknown,
                "trace": {"source": "execution"}
            }));
        }
        let mut replacement =
            ToolResponse::failed(json!({"error": "withheld"})).into_content("charge", None);
        if let Part::FunctionResponse { annotations, .. } = &mut replacement.parts[0] {
            *annotations = Some(json!({
                TOOL_RESULT_STATUS_KEY: ToolResultStatus::Failed,
                "audit": "screened"
            }));
        }
        ToolResponse::preserve_unknown(&previous, &mut replacement);
        assert_eq!(
            replacement,
            Content {
                role: "function".to_string(),
                parts: vec![Part::FunctionResponse {
                    function_response: FunctionResponseData::new(
                        "charge",
                        json!({"error": "withheld"})
                    ),
                    id: None,
                    annotations: Some(json!({
                        TOOL_RESULT_STATUS_KEY: ToolResultStatus::OutcomeUnknown,
                        "trace": {"source": "execution"},
                        "audit": "screened"
                    })),
                }],
            }
        );
    }

    #[test]
    fn known_results_keep_replacements() {
        for status in [
            None,
            Some(ToolResultStatus::Failed),
            Some(ToolResultStatus::Cancelled),
            Some(ToolResultStatus::Pending),
        ] {
            let value = json!({"isError": true, "error": "domain row"});
            let previous = match status {
                Some(status) => ToolResponse::with_status(value.clone(), status),
                None => ToolResponse::new(value.clone()),
            }
            .into_content("charge", None);
            let mut replacement = ToolResponse::new(value).into_content("charge", None);
            let expected = replacement.clone();
            ToolResponse::preserve_unknown(&previous, &mut replacement);
            assert_eq!(replacement, expected);
        }
    }

    #[test]
    fn explicit_annotations_can_resolve_uncertainty() {
        for status in
            [ToolResultStatus::Failed, ToolResultStatus::Cancelled, ToolResultStatus::Pending]
        {
            let mut part = ToolResponse::with_status(Value::Null, ToolResultStatus::OutcomeUnknown)
                .into_part("charge", None);
            ToolResponse::annotate(&mut part, status);
            assert_eq!(part.tool_result_status(), Some(status));
        }
    }
}
