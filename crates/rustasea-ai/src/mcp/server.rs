//! MCP server engine — JSON-RPC 2.0 over newline-delimited frames.
//!
//! The mirror of [`crate::mcp::client::McpClient`]: it answers the handshake
//! (`initialize`), `tools/list` + `tools/call`, `resources/list` +
//! `resources/read`, and `ping`. Frames are read from a generic
//! [`AsyncBufRead`] and written to a generic [`AsyncWrite`], so the same engine
//! drives a child process's stdio (via [`run_stdio`]) and an in-process test
//! with no subprocess.
//!
//! The concrete data surface is injected through [`McpBackend`], keeping this
//! crate free of CLI/application dependencies. Handler failures never abort the
//! loop: a tool error becomes an `isError: true` content frame, a denied
//! resource becomes a JSON-RPC error frame, and a malformed frame becomes a
//! `-32700` parse-error frame — the process never panics.

use serde_json::{json, Value};
use tokio::io::{AsyncBufRead, AsyncBufReadExt, AsyncWrite, AsyncWriteExt};

use crate::error::{AiError, Result};
use crate::mcp::client::MCP_PROTOCOL_VERSION;
use crate::mcp::{McpResource, McpResourceContent, McpTool};

/// Server name reported in the `initialize` response.
pub const SERVER_NAME: &str = "rustasea";

/// JSON-RPC code for a frame that is not valid JSON.
const PARSE_ERROR: i64 = -32700;
/// JSON-RPC code for a method this server does not implement.
const METHOD_NOT_FOUND: i64 = -32601;
/// JSON-RPC code for invalid parameters (e.g. a denied resource URI).
const INVALID_PARAMS: i64 = -32602;

/// Supplies the tools and resources an [`McpServer`] exposes.
///
/// Implementations live in the application/CLI layer so this crate stays free
/// of their dependencies. Every method is infallible at the transport level:
/// handler errors are returned as `String`s and rendered as protocol frames by
/// the server, never as a process exit.
pub trait McpBackend: Send + Sync {
    /// Tools advertised by `tools/list`.
    fn tools(&self) -> Vec<McpTool>;

    /// Invoke `name` with `args`; the `Ok` value is serialized as text content.
    fn call_tool(&self, name: &str, args: Value) -> std::result::Result<Value, String>;

    /// Resources advertised by `resources/list`.
    fn resources(&self) -> Vec<McpResource>;

    /// Read the resource at `uri`; an unknown/denied URI is an `Err`.
    fn read_resource(&self, uri: &str) -> std::result::Result<McpResourceContent, String>;
}

/// A JSON-RPC server driving an [`McpBackend`] over generic async streams.
pub struct McpServer<'a, R, W>
where
    R: AsyncBufRead + Unpin,
    W: AsyncWrite + Unpin,
{
    /// Backend supplying the tool/resource surface.
    backend: &'a dyn McpBackend,
    /// Frame source.
    reader: R,
    /// Frame sink.
    writer: W,
}

impl<'a, R, W> McpServer<'a, R, W>
where
    R: AsyncBufRead + Unpin,
    W: AsyncWrite + Unpin,
{
    /// Build a server over `reader`/`writer` backed by `backend`.
    pub fn new(backend: &'a dyn McpBackend, reader: R, writer: W) -> Self {
        Self {
            backend,
            reader,
            writer,
        }
    }

    /// Read frames until EOF, dispatching each and writing every response.
    ///
    /// Malformed frames produce a `-32700` parse-error frame and the loop
    /// continues; notifications produce no output. Returns when the reader
    /// reaches EOF or a transport error occurs.
    pub async fn serve(&mut self) -> Result<()> {
        let mut line = String::new();
        loop {
            line.clear();
            let read = self.reader.read_line(&mut line).await.map_err(|error| {
                AiError::mcp_server_unreachable(SERVER_NAME, format!("read failed: {error}"))
            })?;
            if read == 0 {
                return Ok(());
            }
            let trimmed = line.trim();
            if trimmed.is_empty() {
                continue;
            }
            let frame: Value = match serde_json::from_str(trimmed) {
                Ok(value) => value,
                Err(error) => {
                    let response = error_frame(
                        Value::Null,
                        PARSE_ERROR,
                        format!("invalid JSON frame: {error}"),
                    );
                    write_frame(&mut self.writer, &response).await?;
                    continue;
                }
            };
            if let Some(response) = self.handle_frame(frame) {
                write_frame(&mut self.writer, &response).await?;
            }
        }
    }

    /// Dispatch one decoded frame; `None` for a notification (no reply).
    ///
    /// The request `id` is echoed verbatim. A frame without an `id` is a
    /// notification and is handled silently, matching JSON-RPC 2.0.
    pub fn handle_frame(&mut self, frame: Value) -> Option<Value> {
        let id = frame.get("id").cloned()?;
        let method = frame
            .get("method")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        let params = frame.get("params").cloned().unwrap_or_else(|| json!({}));
        let response = match method.as_str() {
            "initialize" => success(id, self.initialize_result()),
            "tools/list" => success(id, self.tools_list_result()),
            "tools/call" => self.tools_call_result(id, &params),
            "resources/list" => success(id, self.resources_list_result()),
            "resources/read" => self.resources_read_result(id, &params),
            "ping" => success(id, json!({})),
            other => error_frame(id, METHOD_NOT_FOUND, format!("method not found: {other}")),
        };
        Some(response)
    }

    /// The `initialize` result: protocol version, capabilities, server info.
    fn initialize_result(&self) -> Value {
        json!({
            "protocolVersion": MCP_PROTOCOL_VERSION,
            "capabilities": { "tools": {}, "resources": {} },
            "serverInfo": { "name": SERVER_NAME, "version": env!("CARGO_PKG_VERSION") }
        })
    }

    /// The `tools/list` result.
    fn tools_list_result(&self) -> Value {
        let tools: Vec<Value> = self
            .backend
            .tools()
            .iter()
            .map(|tool| {
                json!({
                    "name": tool.name,
                    "description": tool.description,
                    "inputSchema": tool.input_schema,
                })
            })
            .collect();
        json!({ "tools": tools })
    }

    /// The `tools/call` result, mapping a handler error to `isError: true`.
    fn tools_call_result(&self, id: Value, params: &Value) -> Value {
        let name = params
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let args = params
            .get("arguments")
            .cloned()
            .unwrap_or_else(|| json!({}));
        match self.backend.call_tool(name, args) {
            Ok(value) => success(
                id,
                json!({
                    "content": [{ "type": "text", "text": value.to_string() }],
                    "isError": false
                }),
            ),
            Err(message) => success(
                id,
                json!({
                    "content": [{ "type": "text", "text": message }],
                    "isError": true
                }),
            ),
        }
    }

    /// The `resources/list` result.
    fn resources_list_result(&self) -> Value {
        let resources = self.backend.resources();
        json!({ "resources": resources })
    }

    /// The `resources/read` result; a denied URI becomes an error frame.
    fn resources_read_result(&self, id: Value, params: &Value) -> Value {
        let uri = params
            .get("uri")
            .and_then(Value::as_str)
            .unwrap_or_default();
        match self.backend.read_resource(uri) {
            Ok(content) => success(id, json!({ "contents": [content] })),
            Err(message) => error_frame(id, INVALID_PARAMS, message),
        }
    }
}

/// Run the server over the process's stdio (the `mcp:serve` transport).
///
/// Wraps [`tokio::io::stdin`] in a buffered reader and writes frames to
/// [`tokio::io::stdout`], flushing after each frame so a client sees every
/// response immediately.
pub async fn run_stdio(backend: &dyn McpBackend) -> Result<()> {
    let reader = tokio::io::BufReader::new(tokio::io::stdin());
    let writer = tokio::io::stdout();
    let mut server = McpServer::new(backend, reader, writer);
    server.serve().await
}

/// Build a JSON-RPC success frame.
fn success(id: Value, result: Value) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "result": result })
}

/// Build a JSON-RPC error frame.
fn error_frame(id: Value, code: i64, message: impl Into<String>) -> Value {
    json!({
        "jsonrpc": "2.0",
        "id": id,
        "error": { "code": code, "message": message.into() }
    })
}

/// Serialize `frame` as one newline-delimited JSON-RPC line and flush it.
async fn write_frame<W: AsyncWrite + Unpin>(writer: &mut W, frame: &Value) -> Result<()> {
    let mut bytes = serde_json::to_vec(frame)
        .map_err(|error| AiError::mcp_protocol(SERVER_NAME, error.to_string()))?;
    bytes.push(b'\n');
    writer.write_all(&bytes).await.map_err(|error| {
        AiError::mcp_server_unreachable(SERVER_NAME, format!("write failed: {error}"))
    })?;
    writer.flush().await.map_err(|error| {
        AiError::mcp_server_unreachable(SERVER_NAME, format!("flush failed: {error}"))
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::BufReader;

    /// Backend exposing one tool, one resource, and one denied URI.
    struct Mock;

    impl McpBackend for Mock {
        fn tools(&self) -> Vec<McpTool> {
            vec![McpTool {
                server: SERVER_NAME.to_string(),
                name: "echo".to_string(),
                description: "Echoes text".to_string(),
                input_schema: json!({ "type": "object" }),
            }]
        }

        fn call_tool(&self, name: &str, args: Value) -> std::result::Result<Value, String> {
            match name {
                "echo" => Ok(json!({ "echo": args.get("text").cloned().unwrap_or(Value::Null) })),
                "boom" => Err("tool exploded".to_string()),
                other => Err(format!("unknown tool `{other}`")),
            }
        }

        fn resources(&self) -> Vec<McpResource> {
            vec![McpResource {
                uri: "rustasea://routes".to_string(),
                name: "routes".to_string(),
                description: "Live route table".to_string(),
                mime_type: "application/json".to_string(),
            }]
        }

        fn read_resource(&self, uri: &str) -> std::result::Result<McpResourceContent, String> {
            if uri == "rustasea://routes" {
                Ok(McpResourceContent {
                    uri: uri.to_string(),
                    mime_type: "application/json".to_string(),
                    text: "[]".to_string(),
                })
            } else {
                Err(format!("resource `{uri}` is not in the allow-list"))
            }
        }
    }

    /// Build a server over an empty reader; only `handle_frame` is exercised.
    fn server() -> McpServer<'static, BufReader<&'static [u8]>, Vec<u8>> {
        static BACKEND: Mock = Mock;
        McpServer::new(&BACKEND, BufReader::new(&[][..]), Vec::new())
    }

    #[test]
    fn initialize_reports_protocol_and_capabilities() {
        let mut server = server();
        let response = server
            .handle_frame(json!({ "jsonrpc": "2.0", "id": 1, "method": "initialize" }))
            .expect("initialize replies");
        assert_eq!(response["result"]["protocolVersion"], MCP_PROTOCOL_VERSION);
        assert_eq!(response["result"]["serverInfo"]["name"], SERVER_NAME);
        assert!(response["result"]["capabilities"]["tools"].is_object());
        assert!(response["result"]["capabilities"]["resources"].is_object());
    }

    #[test]
    fn notification_gets_no_reply() {
        let mut server = server();
        let reply = server.handle_frame(json!({
            "jsonrpc": "2.0",
            "method": "notifications/initialized",
            "params": {}
        }));
        assert!(reply.is_none(), "notifications must not be answered");
    }

    #[test]
    fn tools_list_returns_advertised_tools() {
        let mut server = server();
        let response = server
            .handle_frame(json!({ "jsonrpc": "2.0", "id": 2, "method": "tools/list" }))
            .expect("tools/list replies");
        assert_eq!(response["result"]["tools"][0]["name"], "echo");
        assert_eq!(
            response["result"]["tools"][0]["inputSchema"]["type"],
            "object"
        );
    }

    #[test]
    fn tools_call_returns_text_content() {
        let mut server = server();
        let response = server
            .handle_frame(json!({
                "jsonrpc": "2.0",
                "id": 3,
                "method": "tools/call",
                "params": { "name": "echo", "arguments": { "text": "hi" } }
            }))
            .expect("tools/call replies");
        assert_eq!(response["result"]["isError"], false);
        let text = response["result"]["content"][0]["text"]
            .as_str()
            .expect("text content");
        let value: Value = serde_json::from_str(text).expect("text is JSON");
        assert_eq!(value["echo"], "hi");
    }

    #[test]
    fn tool_error_sets_is_error_true() {
        let mut server = server();
        let response = server
            .handle_frame(json!({
                "jsonrpc": "2.0",
                "id": 4,
                "method": "tools/call",
                "params": { "name": "boom", "arguments": {} }
            }))
            .expect("tools/call replies");
        assert_eq!(response["result"]["isError"], true);
        assert!(response["result"]["content"][0]["text"]
            .as_str()
            .expect("text")
            .contains("exploded"));
    }

    #[test]
    fn resources_list_and_read_round_trip() {
        let mut server = server();
        let listed = server
            .handle_frame(json!({ "jsonrpc": "2.0", "id": 5, "method": "resources/list" }))
            .expect("resources/list replies");
        assert_eq!(listed["result"]["resources"][0]["uri"], "rustasea://routes");
        assert_eq!(
            listed["result"]["resources"][0]["mimeType"],
            "application/json"
        );

        let read = server
            .handle_frame(json!({
                "jsonrpc": "2.0",
                "id": 6,
                "method": "resources/read",
                "params": { "uri": "rustasea://routes" }
            }))
            .expect("resources/read replies");
        assert_eq!(read["result"]["contents"][0]["text"], "[]");
    }

    #[test]
    fn denied_resource_is_error_frame() {
        let mut server = server();
        let response = server
            .handle_frame(json!({
                "jsonrpc": "2.0",
                "id": 7,
                "method": "resources/read",
                "params": { "uri": "rustasea://config/../../.env" }
            }))
            .expect("resources/read replies");
        assert!(response.get("error").is_some());
        assert_eq!(response["error"]["code"], INVALID_PARAMS);
    }

    #[test]
    fn unknown_method_is_method_not_found() {
        let mut server = server();
        let response = server
            .handle_frame(json!({ "jsonrpc": "2.0", "id": 8, "method": "bogus/method" }))
            .expect("unknown method replies with an error");
        assert_eq!(response["error"]["code"], METHOD_NOT_FOUND);
    }

    #[test]
    fn ping_returns_empty_result() {
        let mut server = server();
        let response = server
            .handle_frame(json!({ "jsonrpc": "2.0", "id": 9, "method": "ping" }))
            .expect("ping replies");
        assert_eq!(response["result"], json!({}));
    }

    /// A malformed frame yields a parse-error reply rather than a panic.
    #[tokio::test]
    async fn malformed_frame_does_not_panic() {
        static BACKEND: Mock = Mock;
        let input = b"not json\n";
        let reader = BufReader::new(&input[..]);
        let writer: Vec<u8> = Vec::new();
        let mut server = McpServer::new(&BACKEND, reader, writer);
        server.serve().await.expect("serve tolerates a bad frame");
        let output = String::from_utf8(server.writer).expect("utf8 output");
        let frame: Value = serde_json::from_str(output.trim()).expect("valid frame");
        assert_eq!(frame["error"]["code"], PARSE_ERROR);
        assert_eq!(frame["id"], Value::Null);
    }
}
