//! WiringCraft's tools as an MCP server, for an agent the user runs under their own login
//! (Claude Code). The app listens on a local port; the agent starts `wiringcraft-app
//! --mcp-bridge <address>` as its MCP server, which only copies lines between its stdin/stdout
//! and that port. Every tool call is handed to the app's window thread through [`Bridge::poll`],
//! so it runs on the open project exactly like the app's own commands.
//!
//! Only this computer can connect (the port is bound to 127.0.0.1), and a connection must first
//! send the token the app made for this run.

use std::io::{BufRead, BufReader, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::time::Duration;

use serde_json::{Value, json};

use crate::{Entry, Tools};

/// The MCP server's name as the agent sees it; its tools become `mcp__wiringcraft-app__<tool>`.
pub const NAME: &str = "wiringcraft-app";
/// The variable the bridge process reads the token from.
pub const TOKEN_VAR: &str = "WIRINGCRAFT_BRIDGE_TOKEN";
/// The MCP protocol version offered when the agent asks for none we know.
pub const MCP_VERSION: &str = "2025-06-18";
/// How long a tool call waits for the window thread.
const CALL_TIMEOUT: Duration = Duration::from_secs(300);
/// How long a new connection has to send the token.
const TOKEN_TIMEOUT: Duration = Duration::from_secs(10);
/// Connections served at once; more are closed straight away.
const MAX_CONNECTIONS: usize = 8;

/// A tool call waiting for the window thread.
pub struct ToolRequest {
    pub name: String,
    pub input: Value,
    reply: Sender<Result<String, String>>,
    /// Set when the caller stopped waiting, so the call is not run late.
    gave_up: Arc<AtomicBool>,
}

/// The listening side, owned by the app.
pub struct Bridge {
    pub port: u16,
    pub token: String,
    rx: Receiver<ToolRequest>,
}

/// A hard-to-guess token: the standard library's per-process random hash keys, twice.
pub fn new_token() -> String {
    use std::hash::{BuildHasher, Hasher};
    let mut out = String::new();
    for i in 0..2u64 {
        let mut h = std::collections::hash_map::RandomState::new().build_hasher();
        h.write_u64(i);
        h.write_u128(
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0),
        );
        h.write_u32(std::process::id());
        out.push_str(&format!("{:016x}", h.finish()));
    }
    out
}

impl Bridge {
    /// Listen on a free local port. `specs` are the tools offered, `instructions` the server's
    /// note to the agent, `wake` asks the window to run [`Bridge::poll`] soon.
    pub fn start(specs: Vec<Value>, instructions: String, wake: Arc<dyn Fn() + Send + Sync>) -> std::io::Result<Self> {
        let listener = TcpListener::bind(("127.0.0.1", 0))?;
        let port = listener.local_addr()?.port();
        let token = new_token();
        let (tx, rx) = mpsc::channel();
        let shared = Arc::new((specs, instructions, token.clone()));
        let live = Arc::new(AtomicUsize::new(0));
        std::thread::Builder::new().name("wc-bridge".into()).spawn(move || {
            for stream in listener.incoming().map_while(Result::ok) {
                if live.fetch_add(1, Ordering::SeqCst) >= MAX_CONNECTIONS {
                    live.fetch_sub(1, Ordering::SeqCst);
                    continue;
                }
                let (tx, shared, wake, done) = (tx.clone(), Arc::clone(&shared), Arc::clone(&wake), Arc::clone(&live));
                let spawned = std::thread::Builder::new().spawn(move || {
                    serve(stream, &shared, &tx, &*wake);
                    done.fetch_sub(1, Ordering::SeqCst);
                });
                if spawned.is_err() {
                    live.fetch_sub(1, Ordering::SeqCst);
                }
            }
        })?;
        Ok(Self { port, token, rx })
    }

    /// The `mcpServers` entry for the agent's session: this program started as the bridge.
    pub fn server_entry(&self, program: &std::path::Path) -> Value {
        json!({
            "name": NAME,
            "command": program.to_string_lossy(),
            "args": ["--mcp-bridge", format!("127.0.0.1:{}", self.port)],
            "env": [{"name": TOKEN_VAR, "value": self.token}],
        })
    }

    /// Run the tool calls that came in, on the caller's (window) thread. Returns a panel line
    /// for each.
    pub fn poll(&self, tools: &mut dyn Tools) -> Vec<Entry> {
        let mut out = Vec::new();
        while let Ok(req) = self.rx.try_recv() {
            if req.gave_up.load(Ordering::SeqCst) {
                continue;
            }
            let what = tools.describe(&req.name, &req.input);
            let r = tools.run(&req.name, &req.input);
            out.push(Entry::Tool {
                what,
                ok: r.is_ok(),
                detail: match &r {
                    Ok(t) | Err(t) => t.chars().take(300).collect(),
                },
            });
            let _ = req.reply.send(r);
        }
        out
    }
}

/// One connection: check the token, then answer MCP messages line by line.
fn serve(
    stream: TcpStream,
    shared: &(Vec<Value>, String, String),
    tx: &Sender<ToolRequest>,
    wake: &(dyn Fn() + Send + Sync),
) {
    let (specs, instructions, token) = shared;
    let Ok(mut writer) = stream.try_clone() else { return };
    // The token must come quickly; after that the agent may stay quiet as long as it likes.
    if stream.set_read_timeout(Some(TOKEN_TIMEOUT)).is_err() {
        return;
    }
    let Ok(reading) = stream.try_clone() else { return };
    let mut lines = BufReader::new(stream).lines();
    match lines.next() {
        Some(Ok(first)) if first.trim() == token => {}
        _ => return,
    }
    if reading.set_read_timeout(None).is_err() {
        return;
    }
    for line in lines.map_while(Result::ok) {
        let Ok(msg) = serde_json::from_str::<Value>(line.trim()) else {
            continue;
        };
        let reply = handle(&msg, specs, instructions, &mut |name, input| {
            let (rtx, rrx) = mpsc::channel();
            let gave_up = Arc::new(AtomicBool::new(false));
            tx.send(ToolRequest {
                name: name.to_owned(),
                input: input.clone(),
                reply: rtx,
                gave_up: Arc::clone(&gave_up),
            })
            .map_err(|_| "WiringCraft has closed.".to_owned())?;
            wake();
            rrx.recv_timeout(CALL_TIMEOUT).map_err(|_| {
                gave_up.store(true, Ordering::SeqCst);
                "WiringCraft did not answer in time.".to_owned()
            })?
        });
        if let Some(reply) = reply {
            let text = reply.to_string();
            if writer
                .write_all(text.as_bytes())
                .and_then(|()| writer.write_all(b"\n"))
                .and_then(|()| writer.flush())
                .is_err()
            {
                return;
            }
        }
    }
}

/// Answer one MCP message (`None` for a notification). `call` runs a tool.
pub fn handle(
    msg: &Value,
    specs: &[Value],
    instructions: &str,
    call: &mut dyn FnMut(&str, &Value) -> Result<String, String>,
) -> Option<Value> {
    let id = msg.get("id")?.clone();
    let method = msg["method"].as_str().unwrap_or_default();
    let ok = |result: Value| Some(json!({"jsonrpc": "2.0", "id": id, "result": result}));
    match method {
        "initialize" => {
            let version = msg["params"]["protocolVersion"].as_str().unwrap_or(MCP_VERSION);
            ok(json!({
                "protocolVersion": version,
                "capabilities": {"tools": {"listChanged": false}},
                "serverInfo": {"name": NAME, "title": "WiringCraft", "version": env!("CARGO_PKG_VERSION")},
                "instructions": instructions,
            }))
        }
        "ping" => ok(json!({})),
        "tools/list" => {
            let tools: Vec<Value> = specs
                .iter()
                .map(|s| json!({"name": s["name"], "description": s["description"], "inputSchema": s["input_schema"]}))
                .collect();
            ok(json!({"tools": tools}))
        }
        "tools/call" => {
            let name = msg["params"]["name"].as_str().unwrap_or_default();
            let args = match &msg["params"]["arguments"] {
                Value::Null => json!({}),
                a => a.clone(),
            };
            let (text, is_error) = match call(name, &args) {
                Ok(t) => (t, false),
                Err(t) => (t, true),
            };
            ok(json!({"content": [{"type": "text", "text": text}], "isError": is_error}))
        }
        "resources/list" => ok(json!({"resources": []})),
        "prompts/list" => ok(json!({"prompts": []})),
        _ => Some(
            json!({"jsonrpc": "2.0", "id": id, "error": {"code": -32601, "message": format!("no method {method}")}}),
        ),
    }
}

/// The bridge process: send the token, then copy stdin to the app and the app's answers to
/// stdout until either side closes.
pub fn proxy(addr: &str, token: &str) -> std::io::Result<()> {
    let mut up = TcpStream::connect(addr)?;
    up.write_all(token.as_bytes())?;
    up.write_all(b"\n")?;
    let mut down = up.try_clone()?;
    let reader = std::thread::spawn(move || {
        let mut out = std::io::stdout();
        let _ = std::io::copy(&mut down, &mut out);
    });
    let mut input = std::io::stdin();
    let _ = std::io::copy(&mut input, &mut up);
    let _ = up.shutdown(std::net::Shutdown::Write);
    let _ = reader.join();
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn answers_mcp_messages() {
        let specs = vec![json!({"name": "get_state", "description": "d", "input_schema": {"type": "object"}})];
        let mut calls = Vec::new();
        let mut call = |n: &str, a: &Value| {
            calls.push((n.to_owned(), a.clone()));
            if n == "bad" {
                Err("nope".to_owned())
            } else {
                Ok("fine".to_owned())
            }
        };
        let init = handle(
            &json!({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {"protocolVersion": "2025-03-26"}}),
            &specs,
            "note",
            &mut call,
        );
        let init = init.unwrap_or_default();
        assert_eq!(init["result"]["protocolVersion"], "2025-03-26");
        assert_eq!(init["result"]["instructions"], "note");
        assert!(
            handle(
                &json!({"jsonrpc": "2.0", "method": "notifications/initialized"}),
                &specs,
                "",
                &mut call
            )
            .is_none()
        );
        let list = handle(
            &json!({"jsonrpc": "2.0", "id": 2, "method": "tools/list"}),
            &specs,
            "",
            &mut call,
        )
        .unwrap_or_default();
        assert_eq!(list["result"]["tools"][0]["inputSchema"]["type"], "object");
        let r = handle(
            &json!({"jsonrpc": "2.0", "id": 3, "method": "tools/call", "params": {"name": "bad", "arguments": {"x": 1}}}),
            &specs,
            "",
            &mut call,
        )
        .unwrap_or_default();
        assert_eq!(r["result"]["isError"], true);
        assert_eq!(r["result"]["content"][0]["text"], "nope");
        assert_eq!(calls.len(), 1);
    }

    struct Echo;
    impl Tools for Echo {
        fn specs(&self) -> Vec<Value> {
            Vec::new()
        }
        fn run(&mut self, name: &str, input: &Value) -> Result<String, String> {
            Ok(format!("{name} {input}"))
        }
    }

    #[test]
    fn a_tool_call_over_the_socket_runs_on_the_polling_thread() {
        let bridge = Bridge::start(Vec::new(), String::new(), Arc::new(|| {}));
        let Ok(bridge) = bridge else { return };
        let addr = format!("127.0.0.1:{}", bridge.port);
        let token = bridge.token.clone();
        let client = std::thread::spawn(move || -> Option<Value> {
            let mut s = TcpStream::connect(&addr).ok()?;
            s.write_all(format!("{token}\n").as_bytes()).ok()?;
            let msg = json!({"jsonrpc": "2.0", "id": 9, "method": "tools/call", "params": {"name": "show", "arguments": {"page": "2"}}});
            s.write_all(format!("{msg}\n").as_bytes()).ok()?;
            let mut line = String::new();
            BufReader::new(s).read_line(&mut line).ok()?;
            serde_json::from_str(&line).ok()
        });
        let mut entries = Vec::new();
        for _ in 0..400 {
            entries.extend(bridge.poll(&mut Echo));
            if !entries.is_empty() {
                break;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        let reply = client.join().ok().flatten().unwrap_or_default();
        assert_eq!(reply["id"], 9);
        assert_eq!(reply["result"]["content"][0]["text"], "show {\"page\":\"2\"}");
        assert_eq!(entries.len(), 1);
    }

    #[test]
    fn a_call_the_agent_gave_up_on_is_not_run() {
        let (tx, rx) = mpsc::channel();
        let bridge = Bridge {
            port: 0,
            token: String::new(),
            rx,
        };
        let (rtx, _rrx) = mpsc::channel();
        let _ = tx.send(ToolRequest {
            name: "late".into(),
            input: json!({}),
            reply: rtx.clone(),
            gave_up: Arc::new(AtomicBool::new(true)),
        });
        let _ = tx.send(ToolRequest {
            name: "now".into(),
            input: json!({}),
            reply: rtx,
            gave_up: Arc::new(AtomicBool::new(false)),
        });
        let entries = bridge.poll(&mut Echo);
        assert_eq!(entries.len(), 1);
        assert!(matches!(&entries[0], Entry::Tool { detail, .. } if detail.starts_with("now")));
    }

    #[test]
    fn a_silent_connection_is_closed() {
        let Ok(bridge) = Bridge::start(Vec::new(), String::new(), Arc::new(|| {})) else {
            return;
        };
        let Ok(s) = TcpStream::connect(("127.0.0.1", bridge.port)) else {
            return;
        };
        let _ = s.set_read_timeout(Some(TOKEN_TIMEOUT + Duration::from_secs(10)));
        let mut line = String::new();
        let n = BufReader::new(s).read_line(&mut line).unwrap_or(1);
        assert_eq!(n, 0, "closed after the token wait");
    }

    #[test]
    fn a_wrong_token_is_dropped() {
        let Ok(bridge) = Bridge::start(Vec::new(), String::new(), Arc::new(|| {})) else {
            return;
        };
        let Ok(mut s) = TcpStream::connect(("127.0.0.1", bridge.port)) else {
            return;
        };
        let _ = s.write_all(b"guess\n{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"ping\"}\n");
        let _ = s.set_read_timeout(Some(Duration::from_secs(2)));
        let mut line = String::new();
        let n = BufReader::new(s).read_line(&mut line).unwrap_or(0);
        assert_eq!(n, 0, "no answer without the token");
    }
}
