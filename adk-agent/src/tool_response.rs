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

    pub(crate) fn annotate(part: &mut Part, status: ToolResultStatus) {
        if let Part::FunctionResponse { annotations, .. } = part {
            let mut fields =
                annotations.take().and_then(|value| value.as_object().cloned()).unwrap_or_default();
            fields.insert(TOOL_RESULT_STATUS_KEY.to_string(), json!(status));
            *annotations = Some(Value::Object(fields));
        }
    }
}
