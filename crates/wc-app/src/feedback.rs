//! Requests from users into the todo list: a problem, a function WiringCraft should have, or
//! something missing. Each becomes a GitHub issue labelled `todo`, which the project's agents
//! work through in order, so what users ask for arrives in a later version.
//!
//! An issue is filed one of two ways. The Help menu, and the assistant by default, open the
//! issue form in the browser with everything filled in, so the person reads it and presses
//! Submit. When the GitHub command line (`gh`) is installed and signed in and the person has
//! agreed to the text, the assistant can file it directly.

use std::io::Read;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

pub const REPO: &str = "phobicdotno/wiringcraft";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// Something broke or gave a wrong result.
    Problem,
    /// Something WiringCraft should be able to do.
    Function,
    /// A symbol, part, report, setting or step that was expected and is not there.
    Missing,
}

impl Kind {
    pub fn parse(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "problem" | "bug" | "error" => Some(Self::Problem),
            "function" | "feature" | "request" => Some(Self::Function),
            "missing" => Some(Self::Missing),
            _ => None,
        }
    }

    /// The issue form in `.github/ISSUE_TEMPLATE`.
    fn template(self) -> &'static str {
        match self {
            Self::Problem => "bug.yml",
            Self::Function => "feature.yml",
            Self::Missing => "missing.yml",
        }
    }

    /// The word in an issue's request marker; `.github/workflows/requests.yml` labels by it.
    fn marker(self) -> &'static str {
        match self {
            Self::Problem => "problem",
            Self::Function => "function",
            Self::Missing => "missing",
        }
    }

    /// The form's field for steps or an example, if it has one.
    fn detail_field(self) -> Option<&'static str> {
        match self {
            Self::Problem => Some("steps"),
            Self::Function => Some("example"),
            Self::Missing => None,
        }
    }
}

/// One request, as the person or the assistant wrote it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Request {
    pub title: String,
    pub what: String,
    /// Steps to make a problem happen again, or an example of a function in use.
    pub detail: String,
    pub done: String,
}

/// The version and system, for problem reports.
pub fn version() -> String {
    format!(
        "WiringCraft {} on {} {}",
        env!("CARGO_PKG_VERSION"),
        std::env::consts::OS,
        std::env::consts::ARCH
    )
}

/// Percent-encode a query value (RFC 3986 unreserved characters stay as they are).
fn query_escape(s: &str) -> String {
    let mut out = String::new();
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'~') {
            out.push(char::from(b));
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

/// Longest form link, in bytes. GitHub and some browsers refuse links much past 8 KB, so the
/// fields are cut (whole characters) to keep the link under this.
pub const MAX_URL: usize = 6000;

/// The issue form in the browser, with the request filled in.
pub fn form_url(kind: Kind, r: &Request) -> String {
    let mut url = format!("https://github.com/{REPO}/issues/new?template={}", kind.template());
    let mut put = |key: &str, value: &str| {
        let room = MAX_URL.saturating_sub(url.len() + key.len() + 2);
        let mut v = String::new();
        for c in value.trim().chars() {
            let e = query_escape(c.encode_utf8(&mut [0; 4]));
            if v.len() + e.len() > room {
                break;
            }
            v.push_str(&e);
        }
        if !v.is_empty() {
            url.push_str(&format!("&{key}={v}"));
        }
    };
    put("title", &r.title);
    put("what", &r.what);
    if let Some(f) = kind.detail_field() {
        put(f, &r.detail);
    }
    put("done", &r.done);
    if kind == Kind::Problem {
        put("version", &version());
    }
    url
}

/// The issue text, laid out like the form makes it.
pub fn body(kind: Kind, r: &Request) -> String {
    let mut out = String::new();
    let mut section = |heading: &str, text: &str| {
        if !text.trim().is_empty() {
            out.push_str(&format!("### {heading}\n\n{}\n\n", text.trim()));
        }
    };
    match kind {
        Kind::Problem => {
            section("What happened", &r.what);
            section("How to make it happen again", &r.detail);
            section("Version and system", &version());
        }
        Kind::Function => {
            section("What", &r.what);
            section("Example", &r.detail);
        }
        Kind::Missing => section("What is missing", &r.what),
    }
    section("Done when", &r.done);
    out.push_str("_Filed from the WiringCraft assistant._\n");
    // The request workflow labels the issue from this, since GitHub drops labels set by
    // someone without write access to the repository.
    out.push_str(&format!("\n<!-- wiringcraft-request: {} -->\n", kind.marker()));
    out
}

/// How long a `gh` call may take before it is stopped, so a stalled network never holds the
/// window for long.
const GH_TIMEOUT: Duration = Duration::from_secs(20);

fn gh(args: &[&str]) -> Result<String, String> {
    let mut child = Command::new("gh")
        .args(args)
        .env("GH_PROMPT_DISABLED", "1")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("The GitHub command line (gh) is not available: {e}"))?;
    // Read the pipes on their own threads so a full pipe never stalls gh.
    let mut out_pipe = child.stdout.take();
    let mut err_pipe = child.stderr.take();
    let out_reader = std::thread::spawn(move || {
        let mut text = String::new();
        if let Some(p) = out_pipe.as_mut() {
            let _ = p.read_to_string(&mut text);
        }
        text
    });
    let err_reader = std::thread::spawn(move || {
        let mut text = String::new();
        if let Some(p) = err_pipe.as_mut() {
            let _ = p.read_to_string(&mut text);
        }
        text
    });
    let start = Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if start.elapsed() < GH_TIMEOUT => std::thread::sleep(Duration::from_millis(50)),
            Ok(None) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(format!("gh did not answer within {} s.", GH_TIMEOUT.as_secs()));
            }
            Err(e) => return Err(format!("Waiting for gh failed: {e}")),
        }
    };
    let out = out_reader.join().unwrap_or_default();
    let err = err_reader.join().unwrap_or_default();
    if status.success() {
        Ok(out)
    } else {
        Err(err.trim().to_owned())
    }
}

/// Whether `gh` is installed and signed in.
pub fn gh_ready() -> bool {
    gh(&["auth", "status"]).is_ok()
}

/// Open and recently closed todos whose text matches `query`, one per line.
pub fn search(query: &str) -> Result<String, String> {
    let q = query.trim();
    if q.is_empty() {
        return Err("`query` is empty.".into());
    }
    let text = gh(&[
        "issue",
        "list",
        "--repo",
        REPO,
        "--state",
        "all",
        "--limit",
        "15",
        "--search",
        q,
        "--json",
        "number,title,state,labels",
        "--template",
        "{{range .}}#{{.number}} [{{.state}}] {{.title}}\n{{end}}",
    ])?;
    Ok(if text.trim().is_empty() {
        "No matching issues.".into()
    } else {
        text
    })
}

/// File the request as an issue with `gh`. Returns the issue's link.
pub fn file(kind: Kind, r: &Request) -> Result<String, String> {
    let body = body(kind, r);
    let out = gh(&[
        "issue",
        "create",
        "--repo",
        REPO,
        "--title",
        r.title.trim(),
        "--body",
        &body,
    ])?;
    Ok(out.trim().to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn form_links_fill_in_the_fields_of_each_form() {
        let r = Request {
            title: "Arrows & dots".into(),
            what: "Wire ends need arrows".into(),
            detail: "Draw a wire, then …".into(),
            done: "A test".into(),
        };
        let bug = form_url(Kind::Problem, &r);
        assert!(bug.starts_with("https://github.com/phobicdotno/wiringcraft/issues/new?template=bug.yml"));
        assert!(bug.contains("&title=Arrows%20%26%20dots"));
        assert!(bug.contains("&steps=Draw%20a%20wire%2C%20then%20%E2%80%A6"));
        assert!(bug.contains("&version=WiringCraft%20"));
        let feature = form_url(Kind::Function, &r);
        assert!(feature.contains("template=feature.yml") && feature.contains("&example="));
        assert!(!feature.contains("&version="));
        let missing = form_url(
            Kind::Missing,
            &Request {
                title: "x".into(),
                ..Request::default()
            },
        );
        assert!(missing.ends_with("template=missing.yml&title=x"));
        // Long text is cut, by whole characters, so the link stays usable.
        for filler in ["a", "界"] {
            let long = form_url(
                Kind::Problem,
                &Request {
                    title: filler.repeat(3000),
                    what: filler.repeat(3000),
                    detail: filler.repeat(3000),
                    done: filler.repeat(3000),
                },
            );
            assert!(long.len() <= MAX_URL, "{}", long.len());
            assert!(long.contains("&title="));
        }
    }

    #[test]
    fn kinds_and_bodies() {
        assert_eq!(Kind::parse(" Bug "), Some(Kind::Problem));
        assert_eq!(Kind::parse("feature"), Some(Kind::Function));
        assert_eq!(Kind::parse("missing"), Some(Kind::Missing));
        assert_eq!(Kind::parse("other"), None);
        let b = body(
            Kind::Missing,
            &Request {
                title: "t".into(),
                what: "A 4-pole breaker symbol".into(),
                done: "It is in the library".into(),
                ..Request::default()
            },
        );
        assert_eq!(
            b,
            "### What is missing\n\nA 4-pole breaker symbol\n\n### Done when\n\nIt is in the library\n\n_Filed from the WiringCraft assistant._\n\n<!-- wiringcraft-request: missing -->\n"
        );
    }
}
