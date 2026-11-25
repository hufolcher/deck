//! `deck mcp`: an MCP server on stdio, so Claude manages the todo itself:
//! reads items and their live status, adds, closes and moves items, starts
//! work sessions, plans its own runs and changes settings. It edits the same
//! files the app watches, so an open `deck todo` follows at once.
//!
//! Register it once: `claude mcp add --scope user deck -- deck mcp`.

mod tools;

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::value::RawValue;
use std::io::{BufRead, Write};

/// JSON-RPC ids are numbers or strings.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(untagged)]
pub enum Id {
    Number(i64),
    Text(String),
}

#[derive(Deserialize)]
struct Incoming {
    /// None: a notification, never answered.
    id: Option<Id>,
    method: String,
    params: Option<Box<RawValue>>,
}

#[derive(Serialize, Debug, PartialEq)]
pub struct RpcError {
    code: i32,
    message: String,
}

#[derive(Serialize, Debug, PartialEq)]
#[serde(rename_all = "lowercase")]
enum Outcome<T> {
    Result(T),
    Error(RpcError),
}

#[derive(Serialize)]
struct Response<T> {
    jsonrpc: &'static str,
    id: Option<Id>,
    #[serde(flatten)]
    outcome: Outcome<T>,
}

#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase")]
struct Initialize {
    protocol_version: Option<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Initialized {
    protocol_version: String,
    capabilities: Capabilities,
    server_info: ServerInfo,
    instructions: &'static str,
}

#[derive(Serialize)]
struct Capabilities {
    tools: Empty,
}

#[derive(Serialize, Deserialize, Default)]
struct Empty {}

#[derive(Serialize)]
struct ServerInfo {
    name: &'static str,
    version: &'static str,
}

const PROTOCOL: &str = "2025-06-18";

const INSTRUCTIONS: &str = "The user's todo, as `deck todo` shows it. todo_list first; todo_status for \
what Linear, Slack, GitHub and Claude sessions say about an item now. Plan runs with schedule_add \
(they run while `deck todo` is open). Change the todo only when the user asked for it.";

fn params<T: DeserializeOwned + Default>(raw: &Option<Box<RawValue>>) -> Result<T, RpcError> {
    match raw {
        None => Ok(T::default()),
        Some(raw) => serde_json::from_str(raw.get()).map_err(|error| RpcError {
            code: -32602,
            message: format!("invalid params: {error}"),
        }),
    }
}

fn required<T: DeserializeOwned>(raw: &Option<Box<RawValue>>) -> Result<T, RpcError> {
    let raw = raw.as_ref().ok_or(RpcError {
        code: -32602,
        message: "params missing".into(),
    })?;
    serde_json::from_str(raw.get()).map_err(|error| RpcError {
        code: -32602,
        message: format!("invalid params: {error}"),
    })
}

/// One line in, at most one line out.
pub fn answer(line: &str) -> Option<String> {
    let incoming: Incoming = match serde_json::from_str(line) {
        Ok(incoming) => incoming,
        Err(error) => {
            return Some(reply::<Empty>(
                None,
                Outcome::Error(RpcError {
                    code: -32700,
                    message: format!("parse error: {error}"),
                }),
            ));
        }
    };
    let id = incoming.id.clone()?;
    Some(match incoming.method.as_str() {
        "initialize" => {
            let asked = params::<Initialize>(&incoming.params).unwrap_or_default();
            reply(
                Some(id),
                Outcome::Result(Initialized {
                    protocol_version: asked.protocol_version.unwrap_or_else(|| PROTOCOL.into()),
                    capabilities: Capabilities { tools: Empty {} },
                    server_info: ServerInfo {
                        name: "deck",
                        version: env!("CARGO_PKG_VERSION"),
                    },
                    instructions: INSTRUCTIONS,
                }),
            )
        }
        "ping" => reply(Some(id), Outcome::Result(Empty {})),
        "tools/list" => reply(Some(id), Outcome::Result(tools::list())),
        "tools/call" => match required::<tools::Call>(&incoming.params) {
            Ok(call) => reply(Some(id), Outcome::Result(call.run())),
            Err(error) => reply(
                Some(id),
                Outcome::Result(tools::Result::error(error.message)),
            ),
        },
        method => reply::<Empty>(
            Some(id),
            Outcome::Error(RpcError {
                code: -32601,
                message: format!("method not found: {method}"),
            }),
        ),
    })
}

fn reply<T: Serialize>(id: Option<Id>, outcome: Outcome<T>) -> String {
    serde_json::to_string(&Response {
        jsonrpc: "2.0",
        id,
        outcome,
    })
    .unwrap_or_else(|error| {
        format!(r#"{{"jsonrpc":"2.0","id":null,"error":{{"code":-32603,"message":"{error}"}}}}"#)
    })
}

pub fn serve() -> std::io::Result<()> {
    let stdin = std::io::stdin();
    let mut stdout = std::io::stdout();
    for line in stdin.lock().lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        if let Some(text) = answer(&line) {
            writeln!(stdout, "{text}")?;
            stdout.flush()?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{Value, json};

    fn ask(request: Value) -> Value {
        serde_json::from_str(&answer(&request.to_string()).expect("an answer")).unwrap()
    }

    #[test]
    fn handshake_echoes_the_version() {
        let answer = ask(json!({ "jsonrpc": "2.0", "id": 1, "method": "initialize",
            "params": { "protocolVersion": "2025-03-26", "capabilities": {}, "clientInfo": { "name": "claude" } } }));
        assert_eq!(answer["id"], 1);
        assert_eq!(answer["result"]["protocolVersion"], "2025-03-26");
        assert_eq!(answer["result"]["serverInfo"]["name"], "deck");
        assert!(answer["result"]["capabilities"]["tools"].is_object());
    }

    #[test]
    fn notifications_get_no_answer() {
        assert_eq!(
            answer(r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#),
            None
        );
    }

    #[test]
    fn errors_by_the_book() {
        assert_eq!(
            ask(json!({ "jsonrpc": "2.0", "id": "a", "method": "nope" }))["error"]["code"],
            -32601
        );
        let garbled: Value = serde_json::from_str(&answer("{not json").unwrap()).unwrap();
        assert_eq!(garbled["error"]["code"], -32700);
        assert_eq!(garbled["id"], Value::Null);
    }

    #[test]
    fn tools_listed_with_schemas() {
        let answer = ask(json!({ "jsonrpc": "2.0", "id": 2, "method": "tools/list" }));
        let tools = answer["result"]["tools"].as_array().unwrap();
        let names: Vec<&str> = tools
            .iter()
            .map(|tool| tool["name"].as_str().unwrap())
            .collect();
        for name in [
            "todo_list",
            "todo_status",
            "todo_add",
            "todo_done",
            "schedule_add",
            "settings_set",
            "work_start",
        ] {
            assert!(names.contains(&name), "{name}");
        }
        assert!(
            tools
                .iter()
                .all(|tool| tool["inputSchema"]["type"] == "object")
        );
    }

    #[test]
    fn unknown_tool_is_a_tool_error_not_a_protocol_one() {
        let answer = ask(json!({ "jsonrpc": "2.0", "id": 3, "method": "tools/call",
            "params": { "name": "rm_rf", "arguments": {} } }));
        assert_eq!(answer["result"]["isError"], true);
        assert!(
            answer["result"]["content"][0]["text"]
                .as_str()
                .unwrap()
                .contains("rm_rf")
        );
    }
}
