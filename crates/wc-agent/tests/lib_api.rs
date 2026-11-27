//! Public behaviour of the crate root: settings on disk, models, tool workers and running a call to the end.

use serde_json::{Value, json};
use wc_agent::{Backend, Conversation, EFFORTS, Image, MODELS, Pending, Settings, Started, Tools, run_to_end};

#[test]
fn settings_read_from_an_empty_object_are_the_defaults() {
    let Ok(read) = serde_json::from_value::<Settings>(json!({})) else {
        unreachable!()
    };
    assert_eq!(read, Settings::default());
    assert_eq!(read.backend, Backend::ClaudeLogin);
    assert_eq!(read.model, MODELS[0].id);
    assert!(EFFORTS.contains(&read.effort.as_str()));
}

#[test]
fn settings_round_trip_and_write_the_backend_in_snake_case() {
    let settings = Settings {
        backend: Backend::ApiKey,
        api_key: "sk-test".into(),
        max_rounds: 5,
        ..Settings::default()
    };
    let Ok(written) = serde_json::to_value(&settings) else {
        unreachable!()
    };
    assert_eq!(written["backend"], "api_key");
    assert_eq!(written["api_key"], "sk-test");
    // Empty strings are left out of the saved preferences.
    assert!(written.get("agent_command").is_none());
    assert!(written.get("login_model").is_none());
    let Ok(back) = serde_json::from_value::<Settings>(written) else {
        unreachable!()
    };
    assert_eq!(back, settings);
}

#[test]
fn model_ids_are_unique_and_labelled() {
    for (i, m) in MODELS.iter().enumerate() {
        assert!(!m.id.is_empty() && !m.label.is_empty());
        assert!(MODELS.iter().skip(i + 1).all(|o| o.id != m.id), "{} listed twice", m.id);
    }
}

#[test]
fn screenshots_alone_send_no_text_block_and_later_words_join_them() {
    let mut c = Conversation::default();
    let shot = Image {
        media_type: "image/png".into(),
        data: "AAAA".into(),
    };
    c.user_with("", std::slice::from_ref(&shot));
    assert_eq!(c.messages().len(), 1);
    let blocks = c.messages()[0]["content"].as_array().cloned().unwrap_or_default();
    assert_eq!(blocks.len(), 1);
    assert_eq!(blocks[0]["type"], "image");
    assert_eq!(blocks[0]["source"]["media_type"], "image/png");
    c.user("what is this?");
    assert_eq!(c.messages().len(), 1, "a second user message joins the first");
    assert_eq!(
        c.messages()[0]["content"][1],
        json!({"type": "text", "text": "what is this?"})
    );
}

#[test]
fn a_worker_gives_its_result_or_says_it_stopped() {
    assert_eq!(Pending::spawn(|| Ok("done".into())).wait(), Ok("done".into()));
    assert_eq!(Pending::spawn(|| Err("no".into())).wait(), Err("no".into()));
    #[allow(clippy::panic)]
    let crashed = Pending::spawn(|| panic!("worker crashed"));
    assert_eq!(crashed.wait(), Err("The tool stopped without an answer.".into()));
}

/// Tools that keep the trait's defaults, except `slow`, which goes to a worker.
#[derive(Default)]
struct Kit {
    ran: Vec<String>,
    finished: Vec<Result<String, String>>,
}

impl Tools for Kit {
    fn specs(&self) -> Vec<Value> {
        Vec::new()
    }
    fn run(&mut self, name: &str, input: &Value) -> Result<String, String> {
        self.ran.push(name.to_owned());
        Ok(format!("{name} {input}"))
    }
    fn start(&mut self, name: &str, input: &Value) -> Started {
        if name == "slow" {
            return Started::Running(Pending::spawn(|| Ok("from worker".into())));
        }
        Started::Done(self.run(name, input))
    }
    fn finish(&mut self, _name: &str, _input: &Value, result: Result<String, String>) -> Result<String, String> {
        self.finished.push(result.clone());
        result.map(|r| format!("{r}, finished"))
    }
}

/// Only `run`: every other method is the trait's default.
struct Plain;

impl Tools for Plain {
    fn specs(&self) -> Vec<Value> {
        Vec::new()
    }
    fn run(&mut self, name: &str, _input: &Value) -> Result<String, String> {
        Err(format!("{name} failed"))
    }
}

#[test]
fn run_to_end_finishes_worker_calls_and_runs_the_rest_directly() {
    let mut kit = Kit::default();
    assert_eq!(run_to_end(&mut kit, "quick", &json!(1)), Ok("quick 1".into()));
    assert!(kit.finished.is_empty());
    assert_eq!(
        run_to_end(&mut kit, "slow", &json!({})),
        Ok("from worker, finished".into())
    );
    assert_eq!(kit.ran, vec!["quick".to_owned()]);
    assert_eq!(kit.finished, vec![Ok("from worker".to_owned())]);

    let mut plain = Plain;
    assert_eq!(run_to_end(&mut plain, "add", &json!({})), Err("add failed".into()));
    assert_eq!(plain.describe("add", &json!({"page": 2})), "add");
    assert_eq!(plain.finish("add", &json!({}), Ok("kept".into())), Ok("kept".into()));
}
