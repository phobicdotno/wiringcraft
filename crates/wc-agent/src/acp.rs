//! Claude under the user's own login: Claude Code started through its Agent Client Protocol
//! adapter (`claude-agent-acp`), the way SolidCraft's agent chat does it. ACP is JSON-RPC 2.0, one
//! message per line over the adapter's stdin and stdout. WiringCraft hands the session its MCP
//! bridge ([`crate::bridge`]) and turns off Claude Code's own shell and file tools, so the agent
//! works on the open project through WiringCraft's tools. WiringCraft never sees a password or
//! key: Claude Code uses the account it is logged in with.

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::Arc;
use std::sync::mpsc::{self, Receiver, Sender};

use serde_json::{Value, json};

use crate::{Entry, Image};

pub const PROTOCOL_VERSION: u64 = 1;
/// The adapter's install command, shown when none is found.
pub const INSTALL: &str = "npm install -g @agentclientprotocol/claude-agent-acp";
/// How to log in when the adapter says the account is not logged in.
pub const LOGIN_HINT: &str = "Claude Code is not logged in on this computer. Open a terminal, run `claude`, type /login and sign in with your Claude account, then send your message again.";

/// Claude Code's own tools that reach the computer rather than the project: off for the session.
pub const DISALLOWED_TOOLS: &[&str] = &[
    "Bash",
    "BashOutput",
    "KillShell",
    "Read",
    "Write",
    "Edit",
    "MultiEdit",
    "NotebookEdit",
    "Glob",
    "Grep",
    "WebFetch",
    "WebSearch",
    "Task",
];

/// Program names of the Claude Code ACP adapter, newest first.
const PROGRAMS: &[&str] = &["claude-agent-acp", "claude-code-acp"];

/// Folders a desktop app may not have on its PATH but where npm and friends put commands.
pub fn search_dirs() -> Vec<PathBuf> {
    let mut dirs: Vec<PathBuf> = std::env::var_os("PATH")
        .map(|p| std::env::split_paths(&p).collect())
        .unwrap_or_default();
    let home = std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from);
    if let Some(h) = &home {
        for d in [
            ".local/bin",
            ".npm-global/bin",
            ".npm-packages/bin",
            ".volta/bin",
            ".bun/bin",
            ".claude/local",
            "bin",
        ] {
            dirs.push(h.join(d));
        }
    }
    if cfg!(windows) {
        if let Some(a) = std::env::var_os("APPDATA") {
            dirs.push(PathBuf::from(a).join("npm"));
        }
    } else {
        for d in [
            "/opt/homebrew/bin",
            "/usr/local/bin",
            "/home/linuxbrew/.linuxbrew/bin",
            "/usr/bin",
        ] {
            dirs.push(PathBuf::from(d));
        }
    }
    dirs
}

fn names_for(program: &str) -> Vec<String> {
    if cfg!(windows) {
        [".exe", ".cmd", ".bat"]
            .iter()
            .map(|e| format!("{program}{e}"))
            .collect()
    } else {
        vec![program.to_owned()]
    }
}

/// The first of `program` in `dirs`.
pub fn find_in(program: &str, dirs: &[PathBuf]) -> Option<PathBuf> {
    dirs.iter()
        .flat_map(|d| names_for(program).into_iter().map(move |n| d.join(n)))
        .find(|p| p.is_file())
}

/// Split a command line into words. Double or single quotes keep spaces in a word
/// (`"/opt/Claude Tools/claude-agent-acp" --flag`); inside double quotes a backslash makes the
/// next quote or backslash literal. No shell is involved.
pub fn split_command(line: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut word = String::new();
    let mut in_word = false;
    let mut quote: Option<char> = None;
    let mut chars = line.chars().peekable();
    while let Some(c) = chars.next() {
        match (quote, c) {
            (Some(q), c) if c == q => quote = None,
            (Some('"'), '\\') if matches!(chars.peek(), Some('"' | '\\')) => {
                if let Some(n) = chars.next() {
                    word.push(n);
                }
            }
            (Some(_), c) => word.push(c),
            (None, '"' | '\'') => {
                quote = Some(c);
                in_word = true;
            }
            (None, c) if c.is_whitespace() => {
                if in_word {
                    out.push(std::mem::take(&mut word));
                    in_word = false;
                }
            }
            (None, c) => {
                word.push(c);
                in_word = true;
            }
        }
    }
    if in_word {
        out.push(word);
    }
    out
}

/// The command that starts the adapter: the user's own command line when given, else an
/// installed adapter, else `npx` fetching it. `None` when there is no way.
pub fn agent_command(custom: &str) -> Option<Vec<String>> {
    let custom = custom.trim();
    if !custom.is_empty() {
        let argv = split_command(custom);
        return (!argv.is_empty()).then_some(argv);
    }
    let dirs = search_dirs();
    for p in PROGRAMS {
        if let Some(path) = find_in(p, &dirs) {
            return Some(vec![path.to_string_lossy().into_owned()]);
        }
    }
    find_in("npx", &dirs).map(|npx| {
        vec![
            npx.to_string_lossy().into_owned(),
            "-y".into(),
            "@agentclientprotocol/claude-agent-acp".into(),
        ]
    })
}

/// Whether Claude Code looks logged in here (its credentials file). On a Mac the login sits in
/// the keychain, so `false` there only means "can't tell".
pub fn looks_logged_in() -> bool {
    let file = std::env::var_os("CLAUDE_CONFIG_DIR")
        .map(|d| PathBuf::from(d).join(".credentials.json"))
        .or_else(|| {
            std::env::var_os("HOME")
                .or_else(|| std::env::var_os("USERPROFILE"))
                .map(|h| PathBuf::from(h).join(".claude").join(".credentials.json"))
        });
    file.is_some_and(|f| f.is_file())
}

pub fn initialize_params() -> Value {
    json!({
        "protocolVersion": PROTOCOL_VERSION,
        "clientCapabilities": {
            "fs": {"readTextFile": false, "writeTextFile": false},
            "terminal": false,
        },
        "clientInfo": {"name": "wiringcraft", "title": "WiringCraft", "version": env!("CARGO_PKG_VERSION")},
    })
}

/// A prompt's content: the pictures (when the agent takes them), then the words. Returns the
/// blocks and how many pictures were left out; the words then say so, so the model knows.
pub fn prompt_blocks(text: &str, images: &[Image], pictures: bool) -> (Vec<Value>, usize) {
    let mut blocks = Vec::new();
    let mut text = text.to_owned();
    let mut left_out = 0;
    if pictures {
        blocks.extend(
            images
                .iter()
                .map(|i| json!({"type": "image", "mimeType": i.media_type, "data": i.data})),
        );
    } else if !images.is_empty() {
        left_out = images.len();
        text.push_str(&format!(
            "\n\n(The user attached {left_out} screenshot(s), but they could not be passed on. Ask them to describe what it shows.)"
        ));
    }
    blocks.push(json!({"type": "text", "text": text}));
    (blocks, left_out)
}

/// `session/new`: the working folder, WiringCraft's MCP bridge, the system note and the
/// adapter's own options (its shell and file tools off).
pub fn session_new_params(cwd: &Path, bridge: &Value, init: &Value, note: &str) -> Value {
    let mut params = json!({
        "cwd": cwd.to_string_lossy(),
        "mcpServers": [bridge],
        "_meta": {
            "systemPrompt": {"append": note},
            "claudeCode": {"options": {"allowDangerouslySkipPermissions": false, "disallowedTools": DISALLOWED_TOOLS}},
        },
    });
    if init
        .pointer("/agentCapabilities/sessionCapabilities/systemPrompt")
        .is_some_and(|v| !v.is_null())
        && let Some(p) = params.as_object_mut()
    {
        p.insert(
            "systemPrompt".into(),
            json!({"mode": "append", "content": [{"type": "text", "text": note}]}),
        );
    }
    params
}

/// Whether a permission question is about one of WiringCraft's own tools
/// (`mcp__wiringcraft-app__<tool>`, as the Claude adapter names them).
pub fn is_bridge_tool(title: &str) -> bool {
    title
        .strip_prefix("mcp__")
        .and_then(|t| t.strip_prefix(crate::bridge::NAME))
        .and_then(|t| t.strip_prefix("__"))
        .is_some_and(|t| {
            !t.is_empty()
                && t.bytes()
                    .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_')
        })
}

/// The answer to a permission question: WiringCraft's own tools run (the assistant's changes
/// are one undo step), anything else is declined.
pub fn permission_answer(params: &Value) -> (Value, bool) {
    let title = params["toolCall"]["title"].as_str().unwrap_or_default();
    let allow = is_bridge_tool(title);
    let want: &[&str] = if allow {
        &["allow_once", "allow_always"]
    } else {
        &["reject_once", "reject_always"]
    };
    let options = params["options"].as_array().cloned().unwrap_or_default();
    let pick = want
        .iter()
        .find_map(|k| options.iter().find(|o| o["kind"] == *k))
        .and_then(|o| o["optionId"].as_str().map(str::to_owned));
    let outcome = match pick {
        Some(id) => json!({"outcome": {"outcome": "selected", "optionId": id}}),
        None => json!({"outcome": {"outcome": "cancelled"}}),
    };
    (outcome, allow)
}

/// The text of a content block.
fn text_of(c: &Value) -> Option<&str> {
    (c["type"] == "text").then(|| c["text"].as_str()).flatten()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum State {
    Starting,
    Ready,
    Busy,
    Off,
}

enum FromAgent {
    Line(String),
    Closed,
}

#[derive(Clone, PartialEq, Eq)]
enum Pending {
    Initialize,
    SessionNew,
    Prompt,
    /// The model asked for, kept so a late answer records the model it was for.
    SetModel(String),
}

/// A running Claude Code session.
pub struct Acp {
    child: Child,
    to_agent: Sender<String>,
    from_agent: Receiver<FromAgent>,
    stderr: Arc<std::sync::Mutex<Vec<String>>>,
    next_id: u64,
    pending: HashMap<u64, Pending>,
    pub state: State,
    session: Option<String>,
    cwd: PathBuf,
    bridge: Value,
    note: String,
    queued: Option<(String, Vec<Image>)>,
    /// The agent takes pictures in a prompt (`promptCapabilities.image`).
    pictures: bool,
    /// Notes for the panel, shown at the next poll.
    notices: Vec<String>,
    /// The user pressed Stop in this turn: its remaining tool calls are refused.
    stopped: bool,
    /// The agent's name and version.
    pub agent: String,
    /// Models the agent offers, `(id, name)`, and the one in use.
    pub models: Vec<(String, String)>,
    pub model: String,
    /// The model to switch to once the session is up.
    wanted_model: String,
}

/// What a poll found.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct AcpPolled {
    pub changed: bool,
    /// A turn ended (answered, stopped or failed).
    pub finished: bool,
}

impl Acp {
    /// Start the adapter and open a session. `wake` asks the window to poll soon.
    pub fn start(
        argv: &[String],
        cwd: &Path,
        bridge: Value,
        note: &str,
        wanted_model: &str,
        wake: Arc<dyn Fn() + Send + Sync>,
    ) -> Result<Self, String> {
        let (program, args) = argv.split_first().ok_or("No command for Claude Code.")?;
        let mut cmd = Command::new(program);
        cmd.args(args)
            .current_dir(cwd)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            // The logged-in account, not a key that happens to be in the environment.
            .env_remove(crate::KEY_VAR)
            .env_remove(crate::bridge::TOKEN_VAR);
        // A desktop app may start without the shell's PATH; npm shims need `node` on it.
        let mut path: Vec<PathBuf> = search_dirs();
        if let Some(dir) = Path::new(program).parent() {
            path.insert(0, dir.to_path_buf());
        }
        if let Ok(joined) = std::env::join_paths(path) {
            cmd.env("PATH", joined);
        }
        let mut child = cmd.spawn().map_err(|e| format!("Could not start {program}: {e}"))?;
        let (Some(mut stdin), Some(stdout), Some(stderr)) =
            (child.stdin.take(), child.stdout.take(), child.stderr.take())
        else {
            let _ = child.kill();
            return Err("Could not talk to Claude Code.".into());
        };
        let (to_agent, rx) = mpsc::channel::<String>();
        std::thread::spawn(move || {
            for line in rx {
                if stdin
                    .write_all(line.as_bytes())
                    .and_then(|()| stdin.write_all(b"\n"))
                    .and_then(|()| stdin.flush())
                    .is_err()
                {
                    break;
                }
            }
        });
        let (tx, from_agent) = mpsc::channel();
        let w = Arc::clone(&wake);
        std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines().map_while(Result::ok) {
                if tx.send(FromAgent::Line(line)).is_err() {
                    return;
                }
                w();
            }
            let _ = tx.send(FromAgent::Closed);
            w();
        });
        let tail = Arc::new(std::sync::Mutex::new(Vec::new()));
        let t = Arc::clone(&tail);
        std::thread::spawn(move || {
            for line in BufReader::new(stderr).lines().map_while(Result::ok) {
                if let Ok(mut v) = t.lock() {
                    if v.len() >= 20 {
                        v.remove(0);
                    }
                    v.push(line.chars().take(300).collect::<String>());
                }
            }
        });
        let mut me = Self {
            child,
            to_agent,
            from_agent,
            stderr: tail,
            next_id: 1,
            pending: HashMap::new(),
            state: State::Starting,
            session: None,
            cwd: cwd.to_path_buf(),
            bridge,
            note: note.to_owned(),
            queued: None,
            pictures: false,
            notices: Vec::new(),
            stopped: false,
            agent: String::new(),
            models: Vec::new(),
            model: String::new(),
            wanted_model: wanted_model.to_owned(),
        };
        me.request(Pending::Initialize, "initialize", initialize_params());
        Ok(me)
    }

    fn send(&self, v: &Value) {
        let _ = self.to_agent.send(v.to_string());
    }

    fn request(&mut self, kind: Pending, method: &str, params: Value) {
        let id = self.next_id;
        self.next_id += 1;
        self.pending.insert(id, kind);
        self.send(&json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}));
    }

    /// Working on a message (or starting with one waiting).
    /// Whether a tool call from the agent belongs to a turn that is running and not stopped.
    pub fn takes_tools(&self) -> bool {
        self.state == State::Busy && !self.stopped
    }

    pub fn busy(&self) -> bool {
        self.state == State::Busy || (self.state == State::Starting && self.queued.is_some())
    }

    pub fn alive(&self) -> bool {
        self.state != State::Off
    }

    /// Send a message; it waits while the session is still starting.
    pub fn prompt(&mut self, text: &str, images: &[Image]) {
        self.stopped = false;
        match (&self.session, self.state) {
            (Some(s), State::Ready) => {
                let (blocks, left_out) = prompt_blocks(text, images, self.pictures);
                if left_out > 0 {
                    self.notices.push(format!(
                        "This Claude Code version does not take pictures, so {left_out} screenshot(s) were not sent."
                    ));
                }
                let params = json!({"sessionId": s, "prompt": blocks});
                self.request(Pending::Prompt, "session/prompt", params);
                self.state = State::Busy;
            }
            _ => self.queued = Some((text.to_owned(), images.to_vec())),
        }
    }

    /// Ask the agent to stop the turn.
    pub fn cancel(&mut self) {
        self.queued = None;
        self.stopped = true;
        if let Some(s) = &self.session {
            self.send(&json!({"jsonrpc": "2.0", "method": "session/cancel", "params": {"sessionId": s}}));
        }
    }

    /// Use another of the models the agent offered.
    pub fn set_model(&mut self, id: &str) {
        self.wanted_model = id.to_owned();
        if let Some(s) = self.session.clone() {
            self.request(
                Pending::SetModel(id.to_owned()),
                "session/set_model",
                json!({"sessionId": s, "modelId": id}),
            );
        }
    }

    fn stderr_tail(&self) -> String {
        self.stderr.lock().map(|v| v.join("\n")).unwrap_or_default()
    }

    /// Read what the agent said; streamed text goes into `log`.
    pub fn poll(&mut self, log: &mut Vec<Entry>) -> AcpPolled {
        let mut out = AcpPolled::default();
        for n in self.notices.drain(..) {
            log.push(Entry::Note(n));
            out.changed = true;
        }
        while let Ok(msg) = self.from_agent.try_recv() {
            out.changed = true;
            match msg {
                FromAgent::Closed => {
                    if self.state != State::Off {
                        let tail = self.stderr_tail();
                        let auth = tail.to_lowercase().contains("login") || tail.to_lowercase().contains("auth");
                        log.push(Entry::Error(if auth {
                            LOGIN_HINT.to_owned()
                        } else if tail.is_empty() {
                            "Claude Code stopped.".to_owned()
                        } else {
                            format!("Claude Code stopped:\n{tail}")
                        }));
                        out.finished = self.busy() || out.finished;
                        self.state = State::Off;
                    }
                }
                FromAgent::Line(line) => self.line(&line, log, &mut out),
            }
        }
        out
    }

    fn line(&mut self, line: &str, log: &mut Vec<Entry>, out: &mut AcpPolled) {
        let Ok(msg) = serde_json::from_str::<Value>(line.trim()) else {
            return;
        };
        let method = msg["method"].as_str();
        match (method, msg.get("id")) {
            (None, Some(id)) => {
                let Some(kind) = id.as_u64().and_then(|i| self.pending.remove(&i)) else {
                    return;
                };
                match msg.get("error") {
                    Some(e) => self.failed(kind, e, log, out),
                    None => self.answered(kind, &msg["result"], log, out),
                }
            }
            (Some("session/request_permission"), Some(id)) => {
                let (answer, allowed) = permission_answer(&msg["params"]);
                if !allowed {
                    let title = msg["params"]["toolCall"]["title"].as_str().unwrap_or("a tool");
                    log.push(Entry::Note(format!(
                        "Declined {title}: the assistant works through WiringCraft's tools."
                    )));
                }
                self.send(&json!({"jsonrpc": "2.0", "id": id, "result": answer}));
            }
            (Some(m), Some(id)) => {
                self.send(&json!({"jsonrpc": "2.0", "id": id, "error": {"code": -32601, "message": format!("{m} is not offered")}}));
            }
            (Some("session/update"), None) => {
                let u = &msg["params"]["update"];
                match u["sessionUpdate"].as_str() {
                    Some("agent_message_chunk") => {
                        if let Some(t) = text_of(&u["content"]) {
                            match log.last_mut() {
                                Some(Entry::Assistant(s)) => s.push_str(t),
                                _ => log.push(Entry::Assistant(t.to_owned())),
                            }
                        }
                    }
                    Some("tool_call") => {
                        // WiringCraft's own tools are shown when they run; others would be declined.
                        let title = u["title"].as_str().unwrap_or_default();
                        if !is_bridge_tool(title) && !title.is_empty() && u["status"] == "failed" {
                            log.push(Entry::Note(format!("{title} failed.")));
                        }
                    }
                    Some("current_model_update") => {
                        if let Some(m) = u["currentModelId"].as_str() {
                            self.model = m.to_owned();
                        }
                    }
                    _ => {}
                }
            }
            _ => {}
        }
    }

    fn answered(&mut self, kind: Pending, result: &Value, log: &mut Vec<Entry>, out: &mut AcpPolled) {
        match kind {
            Pending::Initialize => {
                self.pictures = result
                    .pointer("/agentCapabilities/promptCapabilities/image")
                    .and_then(Value::as_bool)
                    .unwrap_or(false);
                let info = &result["agentInfo"];
                self.agent = [
                    info["title"].as_str().or(info["name"].as_str()),
                    info["version"].as_str(),
                ]
                .into_iter()
                .flatten()
                .collect::<Vec<_>>()
                .join(" ");
                let params = session_new_params(&self.cwd, &self.bridge, result, &self.note);
                self.request(Pending::SessionNew, "session/new", params);
            }
            Pending::SessionNew => {
                let Some(id) = result["sessionId"].as_str() else {
                    log.push(Entry::Error("Claude Code did not open a session.".into()));
                    self.stop_process();
                    out.finished = true;
                    return;
                };
                self.session = Some(id.to_owned());
                if let Some(m) = result.get("models") {
                    self.model = m["currentModelId"].as_str().unwrap_or_default().to_owned();
                    self.models = m["availableModels"]
                        .as_array()
                        .map(|a| {
                            a.iter()
                                .filter_map(|x| {
                                    let id = x["modelId"].as_str()?.to_owned();
                                    let name = x["name"].as_str().unwrap_or(&id).to_owned();
                                    Some((id, name))
                                })
                                .collect()
                        })
                        .unwrap_or_default();
                }
                self.state = State::Ready;
                let wanted = self.wanted_model.clone();
                if !wanted.is_empty() && wanted != self.model && self.models.iter().any(|(id, _)| *id == wanted) {
                    self.set_model(&wanted);
                }
                if let Some((text, images)) = self.queued.take() {
                    self.prompt(&text, &images);
                }
            }
            Pending::Prompt => {
                self.state = State::Ready;
                out.finished = true;
                match result["stopReason"].as_str() {
                    Some("cancelled") => log.push(Entry::Note("Stopped.".into())),
                    Some("max_tokens") | Some("max_turn_requests") => {
                        log.push(Entry::Note("Paused at its limit. Say \"go on\" to continue.".into()))
                    }
                    Some("refusal") => log.push(Entry::Error("Claude declined this request.".into())),
                    _ => {}
                }
            }
            Pending::SetModel(id) => {
                self.model = id;
            }
        }
    }

    fn failed(&mut self, kind: Pending, e: &Value, log: &mut Vec<Entry>, out: &mut AcpPolled) {
        let code = e["code"].as_i64().unwrap_or(0);
        let message = e["message"].as_str().unwrap_or("error").to_owned();
        let auth =
            code == -32000 || message.to_lowercase().contains("auth") || message.to_lowercase().contains("login");
        match kind {
            Pending::SetModel(_) => log.push(Entry::Note(format!("The model was not changed: {message}"))),
            Pending::Prompt if !auth => {
                self.state = State::Ready;
                out.finished = true;
                log.push(Entry::Error(format!("Claude Code: {message}")));
            }
            _ => {
                log.push(Entry::Error(if auth {
                    LOGIN_HINT.to_owned()
                } else {
                    format!("Claude Code could not start a session: {message}")
                }));
                out.finished = true;
                self.stop_process();
            }
        }
    }

    fn stop_process(&mut self) {
        self.state = State::Off;
        self.queued = None;
        let _ = self.child.kill();
    }
}

impl Drop for Acp {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_wiringcraft_tools_are_allowed() {
        let ask = |title: &str| {
            json!({"toolCall": {"title": title}, "options": [
                {"optionId": "a", "kind": "allow_once"}, {"optionId": "r", "kind": "reject_once"}
            ]})
        };
        let (yes, allowed) = permission_answer(&ask("mcp__wiringcraft-app__place_symbol"));
        assert!(allowed);
        assert_eq!(yes["outcome"]["optionId"], "a");
        let (no, allowed) = permission_answer(&ask("Bash"));
        assert!(!allowed);
        assert_eq!(no["outcome"]["optionId"], "r");
        assert!(!is_bridge_tool("mcp__wiringcraft-app__rm -rf /"));
        assert!(!is_bridge_tool("mcp__other__place_symbol"));
    }

    #[test]
    fn a_command_line_keeps_quoted_spaces() {
        assert_eq!(
            split_command(r#" "/opt/Claude Tools/acp" --flag 'a b' "e\"f" "#),
            vec!["/opt/Claude Tools/acp", "--flag", "a b", "e\"f"]
        );
        assert_eq!(split_command("npx -y pkg"), vec!["npx", "-y", "pkg"]);
        assert_eq!(split_command(r#""""#), vec![""]);
        assert!(split_command("   ").is_empty());
        assert_eq!(
            agent_command(r#""/x y/z" -q"#),
            Some(vec!["/x y/z".to_owned(), "-q".to_owned()])
        );
    }

    #[test]
    fn the_session_gets_the_bridge_and_no_shell() {
        let bridge = json!({"name": "wiringcraft-app", "command": "/x", "args": [], "env": []});
        let p = session_new_params(Path::new("/proj"), &bridge, &json!({}), "note");
        assert_eq!(p["mcpServers"][0]["name"], "wiringcraft-app");
        assert_eq!(p["_meta"]["systemPrompt"]["append"], "note");
        assert!(
            p["_meta"]["claudeCode"]["options"]["disallowedTools"]
                .as_array()
                .is_some_and(|a| a.iter().any(|t| t == "Bash"))
        );
        assert!(p.get("systemPrompt").is_none());
        let offered = json!({"agentCapabilities": {"sessionCapabilities": {"systemPrompt": {}}}});
        assert_eq!(
            session_new_params(Path::new("/p"), &bridge, &offered, "n")["systemPrompt"]["mode"],
            "append"
        );
    }

    #[test]
    fn pictures_go_only_to_an_agent_that_takes_them() {
        let shot = Image {
            media_type: "image/png".into(),
            data: "AAAA".into(),
        };
        let (b, left) = prompt_blocks("look", std::slice::from_ref(&shot), true);
        assert_eq!(left, 0);
        assert_eq!(b[0], json!({"type": "image", "mimeType": "image/png", "data": "AAAA"}));
        assert_eq!(b[1], json!({"type": "text", "text": "look"}));
        let (b, left) = prompt_blocks("look", &[shot], false);
        assert_eq!((left, b.len()), (1, 1));
        assert!(
            b[0]["text"]
                .as_str()
                .is_some_and(|t| t.contains("could not be passed on"))
        );
    }

    /// A stand-in adapter: a shell script that answers initialize, session/new and one prompt.
    #[cfg(unix)]
    #[test]
    fn talks_to_an_agent_over_stdio() {
        let dir = std::env::temp_dir().join(format!("wc-acp-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let script = dir.join("agent.sh");
        let body = r#"#!/bin/sh
read l; echo '{"jsonrpc":"2.0","id":1,"result":{"protocolVersion":1,"agentInfo":{"name":"fake","version":"1"}}}'
read l; echo '{"jsonrpc":"2.0","id":2,"result":{"sessionId":"S","models":{"currentModelId":"default","availableModels":[{"modelId":"default","name":"Default"},{"modelId":"opus","name":"Opus"}]}}}'
read l; echo '{"jsonrpc":"2.0","method":"session/update","params":{"sessionId":"S","update":{"sessionUpdate":"agent_message_chunk","content":{"type":"text","text":"Hel"}}}}'
echo '{"jsonrpc":"2.0","method":"session/update","params":{"sessionId":"S","update":{"sessionUpdate":"agent_message_chunk","content":{"type":"text","text":"lo"}}}}'
echo '{"jsonrpc":"2.0","id":3,"result":{"stopReason":"end_turn"}}'
read l
"#;
        if std::fs::write(&script, body).is_err() {
            return;
        }
        let argv = vec!["sh".to_owned(), script.to_string_lossy().into_owned()];
        let acp = Acp::start(&argv, &dir, json!({}), "note", "", Arc::new(|| {}));
        let Ok(mut acp) = acp else { return };
        acp.prompt("hi", &[]);
        assert!(acp.busy());
        let mut log = Vec::new();
        let mut finished = false;
        for _ in 0..400 {
            finished |= acp.poll(&mut log).finished;
            if finished {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert!(finished);
        assert_eq!(log, vec![Entry::Assistant("Hello".into())]);
        assert_eq!(acp.agent, "fake 1");
        assert_eq!(acp.models.len(), 2);
        assert!(!acp.busy());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
