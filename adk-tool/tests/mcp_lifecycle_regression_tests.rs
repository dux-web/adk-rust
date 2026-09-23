//! Direct calls preserve the connection's input policy and subscriptions.
#![cfg(feature = "mcp")]

use adk_tool::mcp::{ConnectionFactory, McpTaskConfig, McpToolset};
use rmcp::model::*;
use rmcp::service::{RequestContext, RunningService};
use rmcp::{ClientHandler, RoleClient, RoleServer, ServerHandler, ServiceExt};
use serde_json::json;
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};

#[derive(Clone, Default)]
struct InputClient(Arc<AtomicUsize>);

impl ClientHandler for InputClient {
    fn get_info(&self) -> ClientInfo {
        ClientInfo::new(
            ClientCapabilities::builder().enable_elicitation().enable_tasks().build(),
            Implementation::new("input-client", "1.0.0"),
        )
        .with_protocol_version(ProtocolVersion::V_2026_07_28)
    }

    async fn create_elicitation(
        &self,
        _params: ElicitRequestParams,
        context: RequestContext<RoleClient>,
    ) -> Result<ElicitResult, ErrorData> {
        assert_eq!(context.id, NumberOrString::String("answer".into()));
        self.0.fetch_add(1, Ordering::SeqCst);
        Ok(ElicitResult::new(ElicitationAction::Accept).with_content(json!({"name":"Ferris"})))
    }
}

#[derive(Clone, Default)]
struct InputServer(Arc<rmcp::task_manager::TaskManager>);

fn input_request() -> InputRequest {
    InputRequest::Elicitation(ElicitRequest::new(ElicitRequestParams::FormElicitationParams {
        meta: None,
        message: "Name".into(),
        requested_schema: serde_json::from_value(json!({
            "type":"object", "properties":{"name":{"type":"string"}}, "required":["name"]
        }))
        .unwrap(),
    }))
}

impl ServerHandler for InputServer {
    fn get_info(&self) -> ServerInfo {
        ServerInfo::new(ServerCapabilities::builder().enable_tools().enable_tasks().build())
            .with_protocol_version(ProtocolVersion::V_2026_07_28)
    }

    async fn call_tool(
        &self,
        params: CallToolRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> Result<CallToolResponse, ErrorData> {
        if params.name == "task" {
            let task = self.0.spawn(Default::default(), |ctx| {
                Box::pin(async move {
                    let response = ctx.request_input("answer", input_request()).await?;
                    Ok(CallToolResult::structured(response))
                })
            });
            return Ok(CreateTaskResult::new(task).into());
        }
        if let Some(responses) = params.input_responses {
            assert_eq!(params.request_state.as_deref(), Some("round-1"));
            return Ok(CallToolResult::structured(responses["answer"].clone()).into());
        }
        Ok(InputRequiredResult::new(
            Some([("answer".into(), input_request())].into()),
            Some("round-1".into()),
        )
        .into())
    }

    async fn get_task(
        &self,
        params: GetTaskParams,
        _context: RequestContext<RoleServer>,
    ) -> Result<GetTaskResult, ErrorData> {
        Ok(GetTaskResult::new(self.0.get_task(&params.task_id)?))
    }

    async fn update_task(
        &self,
        params: UpdateTaskParams,
        _context: RequestContext<RoleServer>,
    ) -> Result<(), ErrorData> {
        self.0.update_task(&params.task_id, params.input_responses)
    }
}

#[tokio::test]
async fn direct_calls_reuse_the_custom_handler_for_inline_and_task_input() {
    let (server_io, client_io) = tokio::io::duplex(4096);
    tokio::spawn(async move {
        InputServer::default().serve(server_io).await.unwrap().waiting().await.unwrap();
    });
    let handler = InputClient::default();
    let handled = handler.0.clone();
    let client = handler.serve(client_io).await.unwrap();
    let toolset = McpToolset::new(client).with_task_support(
        McpTaskConfig::enabled().poll_interval(std::time::Duration::from_millis(1)),
    );
    for name in ["inline", "task"] {
        let value = toolset.call_tool_value(name, Default::default()).await.unwrap();
        assert_eq!(value["output"]["content"]["name"], "Ferris", "{value}");
    }
    assert_eq!(handled.load(Ordering::SeqCst), 2);
}

#[derive(Clone, Default)]
struct SubscriptionServer {
    subscriptions: Arc<AtomicUsize>,
    calls: Arc<AtomicUsize>,
}

impl ServerHandler for SubscriptionServer {
    fn get_info(&self) -> ServerInfo {
        ServerInfo::new(ServerCapabilities::builder().enable_tools().enable_resources().build())
    }

    #[allow(deprecated)]
    async fn subscribe(
        &self,
        params: SubscribeRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> Result<(), ErrorData> {
        assert_eq!(params.uri, "test://resource");
        self.subscriptions.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }

    async fn call_tool(
        &self,
        _params: CallToolRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> Result<CallToolResponse, ErrorData> {
        if self.calls.fetch_add(1, Ordering::SeqCst) == 0 {
            return Err(ErrorData::internal_error("connection reset", None));
        }
        Ok(CallToolResult::structured(json!({"ok":true})).into())
    }
}

struct Factory(SubscriptionServer);

#[async_trait::async_trait]
impl ConnectionFactory<()> for Factory {
    async fn create_connection(&self) -> Result<RunningService<RoleClient, ()>, String> {
        let (server_io, client_io) = tokio::io::duplex(4096);
        let server = self.0.clone();
        tokio::spawn(async move {
            server.serve(server_io).await.unwrap().waiting().await.unwrap();
        });
        ().serve(client_io).await.map_err(|error| error.to_string())
    }
}

#[tokio::test]
async fn direct_call_reconnection_restores_resource_subscriptions() {
    let server = SubscriptionServer::default();
    let subscriptions = server.subscriptions.clone();
    let factory = Arc::new(Factory(server));
    let client = factory.create_connection().await.unwrap();
    let toolset = McpToolset::new(client).with_connection_factory(factory).with_tool_call_retries();
    toolset.subscribe_resource("test://resource").await.unwrap();
    assert_eq!(subscriptions.load(Ordering::SeqCst), 1);
    let value = toolset.call_tool_value("read", Default::default()).await.unwrap();
    assert_eq!(value["output"], json!({"ok":true}));
    assert_eq!(subscriptions.load(Ordering::SeqCst), 2);
}
