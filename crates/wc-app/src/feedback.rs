//! Requests from users into the todo list: a problem, a function WiringCraft should have, or
//! something missing. Each becomes a GitHub issue labelled `todo`, which the project's agents
//! work through in order, so what users ask for arrives in a later version.
//!
//! An issue is filed one of two ways. The Help menu, and the assistant by default, open the
//! issue form in the browser with everything filled in, so the person reads it and presses
//! Submit. When the GitHub command line (`gh`) is installed and signed in and the person has
//! agreed to the text, the assistant can file it directly.

use std::process::Command;

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

    /// The labels the form gives an issue.
    fn labels(self) -> &'static str {
        match self {
            Self::Problem => "todo,bug",
            Self::Function => "todo,enhancement",
            Self::Missing => "todo",
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

/// Longest text put in a form link, in characters per field; browsers refuse very long links.
const MAX_FIELD: usize = 1500;

/// The issue form in the browser, with the request filled in.
pub fn form_url(kind: Kind, r: &Request) -> String {
    let mut url = format!("https://github.com/{REPO}/issues/new?template={}", kind.template());
    let mut put = |key: &str, value: &str| {
        let v: String = value.trim().chars().take(MAX_FIELD).collect();
        if !v.is_empty() {
            url.push_str(&format!("&{key}={}", query_escape(&v)));
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
    out
}

fn gh(args: &[&str]) -> Result<String, String> {
    let out = Command::new("gh")
        .args(args)
        .env("GH_PROMPT_DISABLED", "1")
        .output()
        .map_err(|e| format!("The GitHub command line (gh) is not available: {e}"))?;
    if out.status.success() {
        Ok(String::from_utf8_lossy(&out.stdout).into_owned())
    } else {
        Err(String::from_utf8_lossy(&out.stderr).trim().to_owned())
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
        "--label",
        kind.labels(),
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
        // Long text is cut so the link stays usable.
        let long = form_url(
            Kind::Missing,
            &Request {
                what: "a".repeat(10_000),
                ..Request::default()
            },
        );
        assert!(long.len() < 2_000);
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
            "### What is missing\n\nA 4-pole breaker symbol\n\n### Done when\n\nIt is in the library\n\n_Filed from the WiringCraft assistant._\n"
        );
    }
}
