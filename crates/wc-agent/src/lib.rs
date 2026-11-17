//! WiringCraft's live assistant. A conversation with Claude (Anthropic's Messages API) where
//! the model works on the open project through tools that the app runs: placing symbols,
//! drawing wires, editing pages and devices, running checks, reports and exports.
//!
//! This crate knows nothing about the editor. It owns the conversation, the HTTP calls (made on
//! a background thread so the window never freezes) and the tool loop; the app supplies the
//! tools through [`Tools`] and runs them on its own thread, so every change shows up on the
//! canvas as it happens. A tool that waits on another program hands back a [`Pending`] from
//! [`Tools::start`] instead; it runs on a worker thread and [`Tools::finish`] applies its result
//! on the window thread, so the window keeps drawing meanwhile.

pub mod acp;
pub mod bridge;

use std::collections::VecDeque;
use std::sync::Arc;
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

pub const API_URL: &str = "https://api.anthropic.com/v1/messages";
pub const API_VERSION: &str = "2026-10-21";
/// Beta that lets the API answer on another model when the chosen one declines a request.
pub const FALLBACK_BETA: &str = "server-side-fallback-2026-07-01";
/// Environment variable that holds the API key; it wins over a key saved in the preferences.
pub const KEY_VAR: &str = "ANTHROPIC_API_KEY";

/// A model offered in the settings.
pub struct ModelInfo {
    pub id: &'static str,
    pub label: &'static str,
    /// Whether the API's server-side fallback is offered for it.
    pub fallbacks: bool,
}

/// The models offered, the default first.
pub const MODELS: &[ModelInfo] = &[
    ModelInfo {
        id: "claude-opus-5-5",
        label: "Claude Opus 5.5",
        fallbacks: true,
    },
    ModelInfo {
        id: "claude-sonnet-5-5",
        label: "Claude Sonnet 5.5 (faster)",
        fallbacks: true,
    },
    ModelInfo {
        id: "claude-fable-5-2",
        label: "Claude Fable 5.2 (most capable)",
        fallbacks: true,
    },
    ModelInfo {
        id: "claude-haiku-5-5",
        label: "Claude Haiku 5.5 (fastest)",
        fallbacks: false,
    },
];

/// Effort levels, least to most thorough.
pub const EFFORTS: &[&str] = &["low", "medium", "high", "xhigh", "max"];

/// How the assistant reaches Claude.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Backend {
    /// Claude Code under the user's own Claude login (subscription), like SolidCraft.
    #[default]
    ClaudeLogin,
    /// The Anthropic API with a key.
    ApiKey,
}

/// The assistant's settings, kept with the user's preferences (never in a project).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub backend: Backend,
    /// Claude login: the command that starts Claude Code's ACP adapter, when not the one found.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub agent_command: String,
    /// Claude login: the model to ask Claude Code for (one it offers); empty keeps its default.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub login_model: String,
    /// API key: the model.
    pub model: String,
    pub effort: String,
    /// The API key, when it is not given by [`KEY_VAR`].
    #[serde(skip_serializing_if = "String::is_empty")]
    pub api_key: String,
    /// Most requests in one turn before the assistant stops and asks to go on.
    pub max_rounds: usize,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            backend: Backend::ClaudeLogin,
            agent_command: String::new(),
            login_model: String::new(),
            model: MODELS[0].id.into(),
            effort: "high".into(),
            api_key: String::new(),
            max_rounds: 60,
        }
    }
}

impl Settings {
    /// The key to use: the environment variable first, then the saved one.
    pub fn key(&self) -> Option<String> {
        std::env::var(KEY_VAR)
            .ok()
            .map(|k| k.trim().to_owned())
            .filter(|k| !k.is_empty())
            .or_else(|| Some(self.api_key.trim().to_owned()).filter(|k| !k.is_empty()))
    }

    fn fallbacks(&self) -> bool {
        MODELS.iter().any(|m| m.id == self.model && m.fallbacks)
    }
}

/// A tool call the model asked for.
#[derive(Debug, Clone, PartialEq)]
pub struct ToolCall {
    pub id: String,
    pub name: String,
    pub input: Value,
}

/// Why the model stopped.
#[derive(Debug, Clone, PartialEq)]
pub enum Stop {
    /// It wants tool results.
    ToolUse,
    /// It is done with the turn.
    EndTurn,
    /// The reply hit the length limit.
    MaxTokens,
    /// The request was declined; the text says why when the API gives a reason.
    Refusal(String),
    Other(String),
}

/// One answer from the model.
#[derive(Debug, Clone, PartialEq)]
pub struct Reply {
    pub text: Vec<String>,
    pub calls: Vec<ToolCall>,
    pub stop: Stop,
    /// The model that answered (another one when a fallback ran).
    pub model: String,
    pub input_tokens: u64,
    pub output_tokens: u64,
}

/// A picture sent with the user's words, such as a screenshot.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Image {
    /// `image/png` or `image/jpeg`.
    pub media_type: String,
    /// The bytes, base64 (standard alphabet, padded).
    pub data: String,
}

/// How a message with pictures reads in the panel: the words, then how many pictures came with them.
pub fn shown_text(text: &str, images: usize) -> String {
    match images {
        0 => text.to_owned(),
        n => {
            let what = if n == 1 {
                "1 screenshot".to_owned()
            } else {
                format!("{n} screenshots")
            };
            if text.is_empty() {
                format!("({what})")
            } else {
                format!("{text}\n({what})")
            }
        }
    }
}

/// The messages so far, exactly as the API needs them back: assistant content (thinking
/// blocks included) is kept unchanged.
#[derive(Debug, Clone, Default)]
pub struct Conversation {
    messages: Vec<Value>,
}

impl Conversation {
    pub fn messages(&self) -> &[Value] {
        &self.messages
    }

    pub fn is_empty(&self) -> bool {
        self.messages.is_empty()
    }

    /// Add the user's words. When the last message is already the user's (tool results of a
    /// turn that was stopped), the text joins it, so roles keep alternating.
    pub fn user(&mut self, text: &str) {
        self.user_with(text, &[]);
    }

    /// The user's words with pictures; the pictures go first, so the words can refer to them.
    pub fn user_with(&mut self, text: &str, images: &[Image]) {
        let mut blocks: Vec<Value> = images
            .iter()
            .map(|i| json!({"type": "image", "source": {"type": "base64", "media_type": i.media_type, "data": i.data}}))
            .collect();
        if !text.is_empty() || blocks.is_empty() {
            blocks.push(json!({"type": "text", "text": text}));
        }
        if let Some(last) = self.messages.last_mut()
            && last["role"] == "user"
            && let Some(content) = last["content"].as_array_mut()
        {
            content.extend(blocks);
            return;
        }
        self.messages.push(json!({"role": "user", "content": blocks}));
    }

    /// The request body for the next answer.
    pub fn body(&self, settings: &Settings, system: &str, tools: &[Value]) -> Value {
        let mut body = json!({
            "model": settings.model,
            "max_tokens": 16000,
            "system": [{"type": "text", "text": system}],
            "tools": tools,
            "messages": self.messages,
            "output_config": {"effort": settings.effort},
            // The tools, the instructions and the conversation so far repeat on every request.
            "cache_control": {"type": "ephemeral"},
        });
        if settings.fallbacks() {
            body["fallbacks"] = json!("default");
        }
        body
    }

    /// Take in an answer from the API.
    pub fn absorb(&mut self, response: &Value) -> Result<Reply, String> {
        let content = response["content"]
            .as_array()
            .ok_or_else(|| format!("unexpected answer from the API: {}", short(&response.to_string())))?;
        let mut text = Vec::new();
        let mut calls = Vec::new();
        for block in content {
            match block["type"].as_str() {
                Some("text") => {
                    if let Some(t) = block["text"].as_str()
                        && !t.trim().is_empty()
                    {
                        text.push(t.to_owned());
                    }
                }
                Some("tool_use") => calls.push(ToolCall {
                    id: block["id"].as_str().unwrap_or_default().to_owned(),
                    name: block["name"].as_str().unwrap_or_default().to_owned(),
                    input: block["input"].clone(),
                }),
                _ => {}
            }
        }
        let stop = match response["stop_reason"].as_str() {
            Some("tool_use") => Stop::ToolUse,
            Some("end_turn") | Some("stop_sequence") => Stop::EndTurn,
            Some("max_tokens") => Stop::MaxTokens,
            Some("refusal") => Stop::Refusal(
                response["stop_details"]["explanation"]
                    .as_str()
                    .unwrap_or_default()
                    .to_owned(),
            ),
            other => Stop::Other(other.unwrap_or("unknown").to_owned()),
        };
        // An empty assistant message is not allowed back, so a placeholder stands in for one.
        let kept = if content.is_empty() {
            json!([{"type": "text", "text": "(no answer)"}])
        } else {
            Value::Array(content.clone())
        };
        self.messages.push(json!({"role": "assistant", "content": kept}));
        Ok(Reply {
            text,
            calls,
            stop,
            model: response["model"].as_str().unwrap_or_default().to_owned(),
            input_tokens: response["usage"]["input_tokens"].as_u64().unwrap_or(0)
                + response["usage"]["cache_read_input_tokens"].as_u64().unwrap_or(0)
                + response["usage"]["cache_creation_input_tokens"].as_u64().unwrap_or(0),
            output_tokens: response["usage"]["output_tokens"].as_u64().unwrap_or(0),
        })
    }

    /// Answer every tool call of the last answer, in one user message.
    pub fn tool_results(&mut self, results: Vec<(String, Result<String, String>)>) {
        if results.is_empty() {
            return;
        }
        let blocks: Vec<Value> = results
            .into_iter()
            .map(|(id, r)| match r {
                Ok(text) => json!({"type": "tool_result", "tool_use_id": id, "content": text}),
                Err(text) => json!({"type": "tool_result", "tool_use_id": id, "content": text, "is_error": true}),
            })
            .collect();
        self.messages.push(json!({"role": "user", "content": blocks}));
    }

    /// After a failed request: take back a last user message that only holds the user's words
    /// (no tool results), so it can be sent again. Returns its text.
    pub fn take_unanswered(&mut self) -> Option<String> {
        let last = self.messages.last()?;
        if last["role"] != "user" {
            return None;
        }
        let blocks = last["content"].as_array()?;
        // Pictures sent with the words go too; the app keeps its own copy to offer again.
        if blocks.iter().any(|b| b["type"] != "text" && b["type"] != "image") {
            return None;
        }
        let text: Vec<&str> = blocks.iter().filter_map(|b| b["text"].as_str()).collect();
        let text = text.join("\n");
        self.messages.pop();
        // Give back the user's own words, without the context added in front.
        Some(match text.split_once("</context>\n\n") {
            Some((head, rest)) if head.starts_with("<context>") => rest.to_owned(),
            _ => text,
        })
    }
}

fn short(s: &str) -> String {
    let mut out: String = s.chars().take(300).collect();
    if out.len() < s.len() {
        out.push('…');
    }
    out
}

/// Sends one request and returns the parsed answer.
pub trait Transport: Send + Sync {
    fn send(&self, key: &str, body: &Value) -> Result<Value, String>;
}

/// The real Messages API over HTTPS. Honours `HTTPS_PROXY`.
pub struct Http {
    agent: ureq::Agent,
    url: String,
}

impl Default for Http {
    fn default() -> Self {
        Self::new(API_URL)
    }
}

impl Http {
    pub fn new(url: &str) -> Self {
        let agent = ureq::AgentBuilder::new()
            .timeout_connect(Duration::from_secs(20))
            .timeout(Duration::from_secs(600))
            .try_proxy_from_env(true)
            .build();
        Self {
            agent,
            url: url.to_owned(),
        }
    }
}

impl Transport for Http {
    fn send(&self, key: &str, body: &Value) -> Result<Value, String> {
        let mut attempt = 0u32;
        loop {
            let mut req = self
                .agent
                .post(&self.url)
                .set("x-api-key", key)
                .set("anthropic-version", API_VERSION)
                .set("content-type", "application/json");
            if body.get("fallbacks").is_some() {
                req = req.set("anthropic-beta", FALLBACK_BETA);
            }
            let wait = match req.send_json(body) {
                Ok(resp) => return resp.into_json::<Value>().map_err(|e| format!("unreadable answer: {e}")),
                Err(ureq::Error::Status(code, resp)) => {
                    let retry_after = resp.header("retry-after").and_then(|v| v.trim().parse::<u64>().ok());
                    let text = resp.into_string().unwrap_or_default();
                    let detail = serde_json::from_str::<Value>(&text)
                        .ok()
                        .and_then(|v| v["error"]["message"].as_str().map(str::to_owned))
                        .unwrap_or_else(|| short(&text));
                    let retryable = matches!(code, 408 | 409 | 429 | 500..=599);
                    if !retryable || attempt >= 3 {
                        return Err(match code {
                            401 => format!("The API key was not accepted ({detail})."),
                            403 => format!("This API key may not use this model or feature ({detail})."),
                            429 => format!("Too many requests right now; try again shortly ({detail})."),
                            _ => format!("The API answered {code}: {detail}"),
                        });
                    }
                    retry_after.unwrap_or(2u64.pow(attempt + 1)).min(30)
                }
                Err(e) => {
                    if attempt >= 3 {
                        return Err(format!("Could not reach the API: {e}"));
                    }
                    2u64.pow(attempt + 1)
                }
            };
            attempt += 1;
            std::thread::sleep(Duration::from_secs(wait));
        }
    }
}

/// The app side: the tools the model may call.
pub trait Tools {
    /// Tool definitions in the Messages API's format.
    fn specs(&self) -> Vec<Value>;
    /// Run one tool. `Ok` is the result text, `Err` an error the model should see.
    fn run(&mut self, name: &str, input: &Value) -> Result<String, String>;
    /// Start one tool. A tool that waits on another program returns [`Started::Running`]; the
    /// caller keeps polling and hands its result to [`Tools::finish`] on the same thread. The
    /// default runs the tool straight away.
    fn start(&mut self, name: &str, input: &Value) -> Started {
        Started::Done(self.run(name, input))
    }
    /// Complete a call that [`Tools::start`] left running, with what its worker returned.
    fn finish(&mut self, name: &str, input: &Value, result: Result<String, String>) -> Result<String, String> {
        let _ = (name, input);
        result
    }
    /// A short line for the panel describing a call, e.g. `place_symbol wc.contact.no on page 2`.
    fn describe(&self, name: &str, input: &Value) -> String {
        let _ = input;
        name.to_owned()
    }
}

/// What [`Tools::start`] did with a call.
pub enum Started {
    /// It ran; this is its result.
    Done(Result<String, String>),
    /// It runs on a worker thread.
    Running(Pending),
}

/// A tool's work running on a worker thread.
pub struct Pending(Receiver<Result<String, String>>);

impl Pending {
    /// Run `work` on a new thread.
    pub fn spawn(work: impl FnOnce() -> Result<String, String> + Send + 'static) -> Self {
        let (tx, rx) = mpsc::channel();
        let spawned = std::thread::Builder::new()
            .name("wiringcraft-tool".into())
            .spawn(move || {
                let _ = tx.send(work());
            });
        if let Err(e) = spawned {
            let (tx, rx) = mpsc::channel();
            let _ = tx.send(Err(format!("Could not start the tool: {e}")));
            return Self(rx);
        }
        Self(rx)
    }

    /// The result, once the work is done.
    pub fn try_take(&self) -> Option<Result<String, String>> {
        match self.0.try_recv() {
            Ok(r) => Some(r),
            Err(TryRecvError::Empty) => None,
            Err(TryRecvError::Disconnected) => Some(Err("The tool stopped without an answer.".into())),
        }
    }

    /// Wait for the result.
    pub fn wait(self) -> Result<String, String> {
        self.0
            .recv()
            .unwrap_or_else(|_| Err("The tool stopped without an answer.".into()))
    }
}

/// Run a call to the end on this thread: start it, wait if it went to a worker, finish it.
pub fn run_to_end(tools: &mut dyn Tools, name: &str, input: &Value) -> Result<String, String> {
    match tools.start(name, input) {
        Started::Done(r) => r,
        Started::Running(p) => {
            let r = p.wait();
            tools.finish(name, input, r)
        }
    }
}

/// The tool calls of one reply, run one after another (a call can depend on the one before).
struct Batch {
    calls: VecDeque<ToolCall>,
    results: Vec<(String, Result<String, String>)>,
    stop: Stop,
    can_go_on: bool,
    /// The call running on a worker thread, with its panel line.
    running: Option<(ToolCall, String, Pending)>,
}

/// A line in the assistant panel.
#[derive(Debug, Clone, PartialEq)]
pub enum Entry {
    User(String),
    Assistant(String),
    /// A tool the assistant ran: what it did and whether it worked; the detail is the result.
    Tool {
        what: String,
        ok: bool,
        detail: String,
    },
    Note(String),
    Error(String),
}

/// What a poll found.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Polled {
    /// Something new to show.
    pub changed: bool,
    /// The turn is over (finished, stopped or failed).
    pub finished: bool,
    /// A tool ran, so the project may have changed.
    pub tools_ran: bool,
}

/// A conversation in progress, driven one poll at a time from the app's frame loop.
pub struct Session {
    pub conversation: Conversation,
    pub log: Vec<Entry>,
    transport: Arc<dyn Transport>,
    pending: Option<Receiver<Result<Value, String>>>,
    /// Tool calls of the last reply still to run or finish.
    batch: Option<Batch>,
    rounds: usize,
    stop: bool,
    /// Text given back after a failed first request, for the input box.
    pub returned: Option<String>,
    pub input_tokens: u64,
    pub output_tokens: u64,
}

impl Session {
    pub fn new(transport: Arc<dyn Transport>) -> Self {
        Self {
            conversation: Conversation::default(),
            log: Vec::new(),
            transport,
            pending: None,
            batch: None,
            rounds: 0,
            stop: false,
            returned: None,
            input_tokens: 0,
            output_tokens: 0,
        }
    }

    pub fn busy(&self) -> bool {
        self.pending.is_some() || self.batch.is_some()
    }

    /// Start a fresh conversation (keeps the transport).
    pub fn clear(&mut self) {
        self.conversation = Conversation::default();
        self.log.clear();
        self.pending = None;
        self.batch = None;
        self.stop = false;
        self.returned = None;
    }

    /// Ask the assistant to stop after the request in flight; its tool calls are not run.
    pub fn stop(&mut self) {
        if self.busy() {
            self.stop = true;
        }
    }

    /// Send the user's words and start a turn. `context` (what is open and selected right now)
    /// goes to the model ahead of the words but is not shown in the panel.
    pub fn send(
        &mut self,
        text: &str,
        images: &[Image],
        context: &str,
        settings: &Settings,
        system: &str,
        tools: &[Value],
    ) -> Result<(), String> {
        let text = text.trim();
        if text.is_empty() && images.is_empty() {
            return Err("Write something first.".into());
        }
        if self.busy() {
            return Err("The assistant is still working.".into());
        }
        let key = settings
            .key()
            .ok_or_else(|| format!("No API key: set {KEY_VAR} or add one in the assistant settings."))?;
        if context.trim().is_empty() {
            self.conversation.user_with(text, images);
        } else {
            self.conversation
                .user_with(&format!("<context>\n{}\n</context>\n\n{text}", context.trim()), images);
        }
        self.log.push(Entry::User(shown_text(text, images.len())));
        self.rounds = 0;
        self.stop = false;
        self.request(key, settings, system, tools);
        Ok(())
    }

    fn request(&mut self, key: String, settings: &Settings, system: &str, tools: &[Value]) {
        let body = self.conversation.body(settings, system, tools);
        let transport = Arc::clone(&self.transport);
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            let _ = tx.send(transport.send(&key, &body));
        });
        self.rounds += 1;
        self.pending = Some(rx);
    }

    /// Look for an answer; run its tools and send the results when the model wants more.
    /// With `block`, wait for the answer and for tools on worker threads (for tests and scripts).
    pub fn poll(&mut self, tools: &mut dyn Tools, settings: &Settings, system: &str, block: bool) -> Polled {
        let mut out = Polled::default();
        if self.batch.is_none() {
            let Some(rx) = &self.pending else {
                return out;
            };
            let got = if block {
                rx.recv().map_err(|_| TryRecvError::Disconnected)
            } else {
                rx.try_recv()
            };
            let result = match got {
                Ok(r) => r,
                Err(TryRecvError::Empty) => return out,
                Err(TryRecvError::Disconnected) => Err("The request was lost.".into()),
            };
            self.pending = None;
            out.changed = true;
            let reply = match result.and_then(|v| self.conversation.absorb(&v)) {
                Ok(r) => r,
                Err(e) => {
                    self.log.push(Entry::Error(e));
                    self.returned = self.conversation.take_unanswered();
                    out.finished = true;
                    return out;
                }
            };
            self.input_tokens += reply.input_tokens;
            self.output_tokens += reply.output_tokens;
            for t in &reply.text {
                self.log.push(Entry::Assistant(t.clone()));
            }
            if !reply.model.is_empty() && reply.model != settings.model {
                self.log.push(Entry::Note(format!("Answered by {}.", reply.model)));
            }
            self.batch = Some(Batch {
                can_go_on: reply.stop == Stop::ToolUse && !self.stop,
                calls: reply.calls.into(),
                results: Vec::new(),
                stop: reply.stop,
                running: None,
            });
        }
        self.run_batch(tools, settings, system, block, &mut out);
        out
    }

    fn log_tool(&mut self, what: String, r: &Result<String, String>) {
        self.log.push(Entry::Tool {
            what,
            ok: r.is_ok(),
            detail: match r {
                Ok(t) | Err(t) => short(t),
            },
        });
    }

    /// Run the reply's tool calls in order. A call on a worker thread pauses the batch until
    /// it is done; then the results go back and the turn goes on or ends.
    fn run_batch(&mut self, tools: &mut dyn Tools, settings: &Settings, system: &str, block: bool, out: &mut Polled) {
        let Some(mut b) = self.batch.take() else {
            return;
        };
        loop {
            if let Some((call, what, pending)) = b.running.take() {
                let raw = if block {
                    pending.wait()
                } else {
                    match pending.try_take() {
                        Some(r) => r,
                        None => {
                            b.running = Some((call, what, pending));
                            self.batch = Some(b);
                            return;
                        }
                    }
                };
                let r = tools.finish(&call.name, &call.input, raw);
                out.changed = true;
                out.tools_ran = true;
                self.log_tool(what, &r);
                b.results.push((call.id, r));
            }
            let Some(call) = b.calls.pop_front() else {
                break;
            };
            if !b.can_go_on || self.stop {
                b.results
                    .push((call.id, Err("Not run: the turn was stopped.".to_owned())));
                continue;
            }
            let what = tools.describe(&call.name, &call.input);
            match tools.start(&call.name, &call.input) {
                Started::Done(r) => {
                    out.tools_ran = true;
                    self.log_tool(what, &r);
                    b.results.push((call.id, r));
                }
                Started::Running(p) => {
                    out.changed = true;
                    b.running = Some((call, what, p));
                }
            }
        }
        let results = b.results;
        self.conversation.tool_results(results);
        match &b.stop {
            Stop::ToolUse if self.stop => {
                self.log.push(Entry::Note("Stopped.".into()));
                out.finished = true;
            }
            Stop::ToolUse if self.rounds >= settings.max_rounds.max(1) => {
                self.log.push(Entry::Note(format!(
                    "Paused after {} steps. Say \"go on\" to continue.",
                    self.rounds
                )));
                out.finished = true;
            }
            Stop::ToolUse => match settings.key() {
                Some(key) => self.request(key, settings, system, &tools.specs()),
                None => {
                    self.log.push(Entry::Error(format!("No API key: set {KEY_VAR}.")));
                    out.finished = true;
                }
            },
            Stop::EndTurn => out.finished = true,
            Stop::MaxTokens => {
                self.log.push(Entry::Note(
                    "The answer was cut off at its length limit. Say \"go on\" to continue.".into(),
                ));
                out.finished = true;
            }
            Stop::Refusal(why) => {
                self.log.push(Entry::Error(if why.is_empty() {
                    "The model declined this request.".into()
                } else {
                    format!("The model declined this request: {why}")
                }));
                out.finished = true;
            }
            Stop::Other(s) => {
                self.log.push(Entry::Note(format!("Stopped ({s}).")));
                out.finished = true;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    /// Answers from a script, and remembers the bodies it was sent.
    struct Script {
        answers: Mutex<Vec<Result<Value, String>>>,
        seen: Mutex<Vec<Value>>,
    }

    impl Transport for Script {
        fn send(&self, _key: &str, body: &Value) -> Result<Value, String> {
            if let Ok(mut s) = self.seen.lock() {
                s.push(body.clone());
            }
            self.answers
                .lock()
                .map_err(|e| e.to_string())?
                .pop()
                .unwrap_or_else(|| Err("script ran out".into()))
        }
    }

    fn script(mut answers: Vec<Result<Value, String>>) -> Arc<Script> {
        answers.reverse();
        Arc::new(Script {
            answers: Mutex::new(answers),
            seen: Mutex::new(Vec::new()),
        })
    }

    #[derive(Default)]
    struct Counter {
        calls: Vec<(String, Value)>,
    }

    impl Tools for Counter {
        fn specs(&self) -> Vec<Value> {
            vec![json!({"name": "add", "description": "add", "input_schema": {"type": "object"}})]
        }
        fn run(&mut self, name: &str, input: &Value) -> Result<String, String> {
            self.calls.push((name.to_owned(), input.clone()));
            if input["fail"] == true {
                Err("bad".into())
            } else {
                Ok("done".into())
            }
        }
    }

    fn settings() -> Settings {
        Settings {
            api_key: "test-key".into(),
            ..Settings::default()
        }
    }

    #[test]
    fn runs_tools_until_the_model_is_done() {
        let t = script(vec![
            Ok(json!({
                "model": "claude-opus-5-5",
                "stop_reason": "tool_use",
                "content": [
                    {"type": "thinking", "thinking": "", "signature": "sig"},
                    {"type": "text", "text": "Placing it."},
                    {"type": "tool_use", "id": "t1", "name": "add", "input": {"x": 1}},
                    {"type": "tool_use", "id": "t2", "name": "add", "input": {"fail": true}}
                ],
                "usage": {"input_tokens": 10, "output_tokens": 5}
            })),
            Ok(json!({
                "model": "claude-opus-5-5",
                "stop_reason": "end_turn",
                "content": [{"type": "text", "text": "Done."}],
                "usage": {"input_tokens": 20, "output_tokens": 3}
            })),
        ]);
        let mut s = Session::new(t.clone());
        let mut tools = Counter::default();
        let set = settings();
        assert!(
            s.send("draw a relay", &[], "page 1", &set, "sys", &tools.specs())
                .is_ok()
        );
        let first = s.poll(&mut tools, &set, "sys", true);
        assert!(first.tools_ran && !first.finished);
        let second = s.poll(&mut tools, &set, "sys", true);
        assert!(second.finished);
        assert_eq!(tools.calls.len(), 2);
        assert_eq!(s.output_tokens, 8);

        let seen = t.seen.lock().map(|v| v.clone()).unwrap_or_default();
        assert_eq!(seen.len(), 2);
        let msgs = seen[1]["messages"].as_array().cloned().unwrap_or_default();
        // user, assistant (thinking kept unchanged), tool results in one message
        assert_eq!(msgs.len(), 3);
        assert_eq!(msgs[1]["content"][0]["signature"], "sig");
        let results = msgs[2]["content"].as_array().cloned().unwrap_or_default();
        assert_eq!(results.len(), 2);
        assert_eq!(results[1]["is_error"], true);
        assert_eq!(seen[0]["fallbacks"], "default");
        assert_eq!(seen[0]["output_config"]["effort"], "high");
        assert!(matches!(s.log.last(), Some(Entry::Assistant(t)) if t == "Done."));
    }

    /// `slow` runs on a worker thread until `go` is set; everything else runs at once.
    struct Slow {
        go: Arc<std::sync::atomic::AtomicBool>,
        order: Vec<String>,
    }

    impl Tools for Slow {
        fn specs(&self) -> Vec<Value> {
            Vec::new()
        }
        fn run(&mut self, name: &str, _input: &Value) -> Result<String, String> {
            self.order.push(format!("run {name}"));
            Ok(name.to_owned())
        }
        fn start(&mut self, name: &str, input: &Value) -> Started {
            if name != "slow" {
                return Started::Done(self.run(name, input));
            }
            self.order.push("start slow".into());
            let go = Arc::clone(&self.go);
            Started::Running(Pending::spawn(move || {
                while !go.load(std::sync::atomic::Ordering::SeqCst) {
                    std::thread::sleep(Duration::from_millis(2));
                }
                Ok("slow output".into())
            }))
        }
        fn finish(&mut self, name: &str, _input: &Value, r: Result<String, String>) -> Result<String, String> {
            self.order.push(format!("finish {name}"));
            r.map(|t| format!("{t}, read back"))
        }
    }

    #[test]
    fn a_slow_tool_does_not_hold_up_the_poll() {
        let t = script(vec![
            Ok(json!({
                "stop_reason": "tool_use",
                "content": [
                    {"type": "tool_use", "id": "t1", "name": "slow", "input": {}},
                    {"type": "tool_use", "id": "t2", "name": "quick", "input": {}}
                ]
            })),
            Ok(json!({"stop_reason": "end_turn", "content": [{"type": "text", "text": "Done."}]})),
        ]);
        let mut s = Session::new(t.clone());
        let go = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let mut tools = Slow {
            go: Arc::clone(&go),
            order: Vec::new(),
        };
        let set = settings();
        assert!(s.send("go", &[], "", &set, "sys", &[]).is_ok());
        // Wait for the model's answer; the slow tool then starts and the poll returns.
        let mut started = false;
        for _ in 0..2000 {
            let p = s.poll(&mut tools, &set, "sys", false);
            assert!(!p.finished);
            if !tools.order.is_empty() {
                started = true;
                break;
            }
            std::thread::sleep(Duration::from_millis(1));
        }
        assert!(started);
        // Polls come straight back while the tool works, and the next call waits its turn.
        for _ in 0..5 {
            let p = s.poll(&mut tools, &set, "sys", false);
            assert!(!p.finished && !p.tools_ran);
        }
        assert!(s.busy());
        assert_eq!(tools.order, vec!["start slow"]);
        go.store(true, std::sync::atomic::Ordering::SeqCst);
        let mut finished = false;
        for _ in 0..2000 {
            if s.poll(&mut tools, &set, "sys", false).finished {
                finished = true;
                break;
            }
            std::thread::sleep(Duration::from_millis(1));
        }
        assert!(finished);
        assert_eq!(tools.order, vec!["start slow", "finish slow", "run quick"]);
        let seen = t.seen.lock().map(|v| v.clone()).unwrap_or_default();
        let results = seen[1]["messages"][2]["content"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        assert_eq!(results.len(), 2);
        assert_eq!(results[0]["tool_use_id"], "t1");
        assert!(results[0]["content"].to_string().contains("slow output, read back"));
    }

    #[test]
    fn stopping_while_a_slow_tool_runs_skips_the_rest() {
        let t = script(vec![Ok(json!({
            "stop_reason": "tool_use",
            "content": [
                {"type": "tool_use", "id": "t1", "name": "slow", "input": {}},
                {"type": "tool_use", "id": "t2", "name": "quick", "input": {}}
            ]
        }))]);
        let mut s = Session::new(t);
        let go = Arc::new(std::sync::atomic::AtomicBool::new(true));
        let mut tools = Slow { go, order: Vec::new() };
        let set = settings();
        assert!(s.send("go", &[], "", &set, "sys", &[]).is_ok());
        while tools.order.is_empty() {
            s.poll(&mut tools, &set, "sys", false);
            std::thread::sleep(Duration::from_millis(1));
        }
        s.stop();
        let p = s.poll(&mut tools, &set, "sys", true);
        assert!(p.finished);
        // The started call is finished; the one after it is not run.
        assert_eq!(tools.order, vec!["start slow", "finish slow"]);
        assert!(!s.busy());
    }

    #[test]
    fn screenshots_go_ahead_of_the_words() {
        let t = script(vec![Err("offline".into())]);
        let mut s = Session::new(t);
        let set = settings();
        let shot = Image {
            media_type: "image/png".into(),
            data: "iVBORw0KGgo=".into(),
        };
        assert!(s.send("", &[], "", &set, "sys", &[]).is_err());
        assert!(
            s.send("  ", std::slice::from_ref(&shot), "page 1", &set, "sys", &[])
                .is_ok()
        );
        let msg = &s.conversation.messages()[0]["content"];
        assert_eq!(msg[0]["type"], "image");
        assert_eq!(msg[0]["source"]["media_type"], "image/png");
        assert_eq!(msg[0]["source"]["data"], "iVBORw0KGgo=");
        assert!(msg[1]["text"].as_str().is_some_and(|t| t.starts_with("<context>")));
        assert_eq!(s.log, vec![Entry::User("(1 screenshot)".into())]);
        assert_eq!(shown_text("look", 2), "look\n(2 screenshots)");
        // A failed request gives the words back and leaves no half message behind.
        let p = s.poll(&mut Counter::default(), &set, "sys", true);
        assert!(p.finished);
        assert_eq!(s.returned.as_deref(), Some(""));
        assert!(s.conversation.is_empty());
    }

    #[test]
    fn a_failed_first_request_gives_the_text_back() {
        let t = script(vec![Err("offline".into())]);
        let mut s = Session::new(t);
        let mut tools = Counter::default();
        let set = settings();
        assert!(s.send("hello", &[], "page 2", &set, "sys", &[]).is_ok());
        let p = s.poll(&mut tools, &set, "sys", true);
        assert!(p.finished);
        assert_eq!(s.returned.as_deref(), Some("hello"));
        assert!(s.conversation.is_empty());
    }

    #[test]
    fn stopping_answers_calls_without_running_them() {
        let t = script(vec![Ok(json!({
            "stop_reason": "tool_use",
            "content": [{"type": "tool_use", "id": "t1", "name": "add", "input": {}}]
        }))]);
        let mut s = Session::new(t);
        let mut tools = Counter::default();
        let set = settings();
        assert!(s.send("go", &[], "", &set, "sys", &[]).is_ok());
        s.stop();
        let p = s.poll(&mut tools, &set, "sys", true);
        assert!(p.finished);
        assert!(tools.calls.is_empty());
        // The next words join the tool-result message so roles keep alternating.
        s.conversation.user("again");
        let last = s.conversation.messages().last().cloned().unwrap_or_default();
        assert_eq!(last["content"].as_array().map(Vec::len), Some(2));
    }

    #[test]
    fn haiku_gets_no_fallbacks() {
        let set = Settings {
            model: "claude-haiku-5-5".into(),
            ..settings()
        };
        let body = Conversation::default().body(&set, "s", &[]);
        assert!(body.get("fallbacks").is_none());
    }
}
