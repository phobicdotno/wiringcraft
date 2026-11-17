//! WiringCraft's own log: what happened in the program, with time and level, in a few rotating
//! files in the user's data folder. When something goes wrong the person can look at it and
//! choose to send it with a problem report (Help, Report a problem); nothing is sent otherwise.
//!
//! Lines are masked before they are written: the home folder becomes `~`, and API keys, tokens
//! and email addresses are replaced, so a log can be shared without leaking them. Project content
//! is not saved in the log, but messages WiringCraft shows can name devices or pages, and those
//! messages are logged, so the report dialog asks the person to look the log over first.

use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};
use std::time::{SystemTime, UNIX_EPOCH};

/// Size of one log file before it is rotated.
pub const MAX_FILE: u64 = 1024 * 1024;
/// Rotated files kept besides the current one: `wiringcraft.log.1` (newest) to `.4` (oldest).
/// With [`MAX_FILE`] that caps the log at 5 MB.
pub const KEEP: usize = 4;
const NAME: &str = "wiringcraft.log";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Level {
    Info,
    Warn,
    Error,
}

impl Level {
    fn as_str(self) -> &'static str {
        match self {
            Self::Info => "INFO",
            Self::Warn => "WARN",
            Self::Error => "ERROR",
        }
    }
}

/// A rotating log in one folder.
#[derive(Debug)]
pub struct Logger {
    dir: PathBuf,
    max_file: u64,
    keep: usize,
    home: Option<String>,
    file: Option<File>,
    size: u64,
    /// Why the log could not be written last time, if it could not.
    error: Option<String>,
}

impl Logger {
    pub fn new(dir: impl Into<PathBuf>, max_file: u64, keep: usize) -> Self {
        let home = std::env::var("HOME")
            .or_else(|_| std::env::var("USERPROFILE"))
            .ok()
            .filter(|h| h.len() > 1);
        Self {
            dir: dir.into(),
            max_file,
            keep,
            home,
            file: None,
            size: 0,
            error: None,
        }
    }

    pub fn path(&self) -> PathBuf {
        self.dir.join(NAME)
    }

    fn rotated(&self, n: usize) -> PathBuf {
        self.dir.join(format!("{NAME}.{n}"))
    }

    pub fn write(&mut self, level: Level, message: &str) {
        let line = format!(
            "{} {:5} {}\n",
            timestamp(SystemTime::now()),
            level.as_str(),
            mask(message, self.home.as_deref())
        );
        // A log that cannot be written must never stop the program. Keep the reason, and say
        // it on stderr once per new reason (not through the log, which is what failed).
        match self.append(&line) {
            Ok(()) => self.error = None,
            Err(e) => {
                let reason = format!("{}: {e}", self.dir.display());
                let new = self.error.as_deref() != Some(reason.as_str());
                self.error = Some(reason);
                if new && let Some(r) = &self.error {
                    // `eprintln!` would panic if stderr is gone; a failed note is ignored.
                    let _ = writeln!(std::io::stderr(), "WiringCraft could not write its log in {r}");
                }
            }
        }
    }

    /// Why the last line could not be written, if it could not.
    pub fn error(&self) -> Option<&str> {
        self.error.as_deref()
    }

    fn append(&mut self, line: &str) -> std::io::Result<()> {
        // One line never makes a file bigger than the limit: cut it at a character boundary.
        let cut;
        let line = if line.len() as u64 > self.max_file {
            let mut end = usize::try_from(self.max_file.saturating_sub(1))
                .unwrap_or(0)
                .min(line.len());
            while !line.is_char_boundary(end) {
                end -= 1;
            }
            cut = format!("{}\n", &line[..end]);
            cut.as_str()
        } else {
            line
        };
        if self.file.is_none() {
            fs::create_dir_all(&self.dir)?;
            let f = OpenOptions::new().create(true).append(true).open(self.path())?;
            self.size = f.metadata()?.len();
            self.file = Some(f);
        }
        if self.size > 0 && self.size + line.len() as u64 > self.max_file {
            self.rotate()?;
        }
        if let Some(f) = self.file.as_mut() {
            f.write_all(line.as_bytes())?;
            self.size += line.len() as u64;
        }
        Ok(())
    }

    /// Current file becomes `.1`, `.1` becomes `.2`, ..., the oldest is dropped.
    fn rotate(&mut self) -> std::io::Result<()> {
        self.file = None;
        if self.keep == 0 {
            remove_if_there(&self.path())?;
        } else {
            remove_if_there(&self.rotated(self.keep))?;
            for n in (1..self.keep).rev() {
                let from = self.rotated(n);
                if from.exists() {
                    fs::rename(&from, self.rotated(n + 1))?;
                }
            }
            fs::rename(self.path(), self.rotated(1))?;
        }
        self.file = Some(OpenOptions::new().create(true).append(true).open(self.path())?);
        self.size = 0;
        Ok(())
    }

    /// The newest `max_bytes` of the log, across rotated files, oldest line first. Starts at a
    /// whole line.
    pub fn tail(&self, max_bytes: usize) -> String {
        let mut parts = Vec::new();
        let mut total = 0usize;
        let files = std::iter::once(self.path()).chain((1..=self.keep).map(|n| self.rotated(n)));
        for path in files {
            let Ok(text) = fs::read_to_string(&path) else { continue };
            total += text.len();
            parts.push(text);
            if total >= max_bytes {
                break;
            }
        }
        parts.reverse();
        let all = parts.concat();
        if all.len() <= max_bytes {
            return all;
        }
        let mut start = all.len() - max_bytes;
        while !all.is_char_boundary(start) {
            start += 1;
        }
        match all[start..].find('\n') {
            Some(i) => all[start + i + 1..].to_owned(),
            None => all[start..].to_owned(),
        }
    }
}

/// Delete a file; one that is already gone is fine, any other failure is passed on.
fn remove_if_there(path: &std::path::Path) -> std::io::Result<()> {
    match fs::remove_file(path) {
        Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e),
        _ => Ok(()),
    }
}

/// `YYYY-MM-DDTHH:MM:SSZ` in UTC.
fn timestamp(t: SystemTime) -> String {
    let secs = t.duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    let (days, rem) = (secs / 86_400, secs % 86_400);
    // Civil date from days since 1970-01-01 (Howard Hinnant's algorithm).
    let z = days as i64 + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!(
        "{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}Z",
        rem / 3600,
        rem % 3600 / 60,
        rem % 60
    )
}

/// Prefixes of secrets that are replaced with the prefix and `***`.
const SECRET_PREFIXES: &[&str] = &[
    "sk-ant-",
    "sk-",
    "ghp_",
    "gho_",
    "ghu_",
    "ghs_",
    "ghr_",
    "github_pat_",
    "xoxb-",
    "xoxp-",
    "AKIA",
];
/// Words after which the next token is a secret (`api_key=...`, `Bearer ...`).
const SECRET_KEYS: &[&str] = &[
    "api_key",
    "apikey",
    "api-key",
    "token",
    "password",
    "secret",
    "bearer",
    "authorization",
];

fn is_token_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.' | '+' | '/' | '=')
}

/// Where a secret value that continues in `rest` ends: at the next white space.
fn value_end(rest: &str) -> usize {
    rest.find(char::is_whitespace).unwrap_or(rest.len())
}

/// Hide what must not leave the machine: the home folder (as `~`), API keys and tokens, values
/// after key words such as `token=` or `Bearer`, and email addresses.
pub fn mask(text: &str, home: Option<&str>) -> String {
    let mut s = text.replace(['\n', '\r'], " ");
    if let Some(h) = home {
        s = s.replace(h, "~");
        let alt = h.replace('\\', "/");
        if alt != h {
            s = s.replace(&alt, "~");
        }
    }
    let mut out = String::with_capacity(s.len());
    let mut hide_next = false;
    let mut rest = s.as_str();
    while !rest.is_empty() {
        let start = rest.find(is_token_char).unwrap_or(rest.len());
        out.push_str(&rest[..start]);
        rest = &rest[start..];
        let end = rest.find(|c: char| !is_token_char(c) && c != '@').unwrap_or(rest.len());
        let word = &rest[..end];
        rest = &rest[end..];
        if word.is_empty() {
            continue;
        }
        let lower = word.to_ascii_lowercase();
        let key = SECRET_KEYS
            .iter()
            .find(|k| lower.starts_with(*k) && (lower.len() == k.len() || lower[k.len()..].starts_with(['=', ':'])));
        if let Some(p) = SECRET_PREFIXES
            .iter()
            .find(|p| word.starts_with(*p) && word.len() >= p.len() + 8)
        {
            out.push_str(p);
            out.push_str("***");
            hide_next = false;
        } else if let Some(k) = key {
            match word.find(['=', ':']) {
                // `token=abc...` in one word: keep the key, hide the value up to the next space
                // (a value can hold characters that end an ordinary word, such as `!`).
                Some(i) if word.len() > i + 1 && i >= k.len() => {
                    out.push_str(&word[..=i]);
                    out.push_str("***");
                    rest = &rest[value_end(rest)..];
                }
                _ => {
                    out.push_str(word);
                    hide_next = true;
                }
            }
        } else if hide_next {
            // The value after a key word, whatever its length, up to the next space.
            out.push_str("***");
            rest = &rest[value_end(rest)..];
            hide_next = false;
        } else if word.find('@').is_some_and(|at| at > 0 && word[at + 1..].contains('.')) {
            out.push_str("<email>");
            hide_next = false;
        } else {
            out.push_str(word);
            hide_next = false;
        }
    }
    out
}

static LOG: OnceLock<Mutex<Logger>> = OnceLock::new();

/// WiringCraft's data folder for logs.
pub fn dir() -> Option<PathBuf> {
    crate::prefs::config_dir().map(|d| d.join("logs"))
}

/// Start logging into the data folder and log panics. Safe to call more than once.
pub fn init() {
    let Some(dir) = dir() else { return };
    if LOG.set(Mutex::new(Logger::new(dir, MAX_FILE, KEEP))).is_err() {
        return;
    }
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let place = info
            .location()
            .map(|l| format!(" at {}:{}", l.file(), l.line()))
            .unwrap_or_default();
        let what = info
            .payload()
            .downcast_ref::<&str>()
            .map(|s| (*s).to_owned())
            .or_else(|| info.payload().downcast_ref::<String>().cloned())
            .unwrap_or_else(|| "unknown".into());
        log(Level::Error, &format!("panic{place}: {what}"));
        previous(info);
    }));
    log(Level::Info, &format!("start {}", crate::feedback::version()));
}

pub fn log(level: Level, message: &str) {
    if let Some(m) = LOG.get()
        && let Ok(mut l) = m.lock()
    {
        l.write(level, message);
    }
}

pub fn info(message: &str) {
    log(Level::Info, message);
}

pub fn warn(message: &str) {
    log(Level::Warn, message);
}

/// Why the log could not be written, if it could not.
pub fn error() -> Option<String> {
    LOG.get()
        .and_then(|m| m.lock().ok().and_then(|l| l.error().map(str::to_owned)))
}

/// The newest part of the log, for a problem report.
pub fn tail(max_bytes: usize) -> String {
    LOG.get()
        .and_then(|m| m.lock().ok().map(|l| l.tail(max_bytes)))
        .unwrap_or_default()
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn rotates_at_the_size_limit_and_keeps_a_few_files() {
        let tmp = tempfile::tempdir().unwrap();
        let mut l = Logger::new(tmp.path(), 200, 2);
        for i in 0..40 {
            l.write(Level::Info, &format!("line {i:02} with some padding text"));
        }
        let size = |p: PathBuf| fs::metadata(p).map(|m| m.len()).unwrap_or(0);
        assert!(size(l.path()) <= 200);
        assert!(size(l.rotated(1)) <= 200 && size(l.rotated(1)) > 0);
        assert!(size(l.rotated(2)) > 0);
        assert!(!l.rotated(3).exists(), "only two rotated files are kept");
        let tail = l.tail(10_000);
        assert!(tail.trim_end().ends_with("line 39 with some padding text"));
        assert!(!tail.contains("line 00 "), "the oldest lines were dropped");
        // Lines in order, oldest first.
        let nums: Vec<u32> = tail
            .lines()
            .filter_map(|l| l.split("line ").nth(1)?.get(..2)?.parse().ok())
            .collect();
        assert!(nums.windows(2).all(|w| w[0] + 1 == w[1]), "{nums:?}");
        let short = l.tail(80);
        assert!(short.len() <= 80 && short.ends_with('\n') && short.contains("line 39"));
    }

    #[test]
    fn one_long_line_never_passes_the_size_limit() {
        let tmp = tempfile::tempdir().unwrap();
        let mut l = Logger::new(tmp.path(), 100, 2);
        l.write(Level::Info, &"æøå".repeat(100));
        l.write(Level::Info, "short");
        l.write(Level::Info, &"x".repeat(500));
        for p in [l.path(), l.rotated(1), l.rotated(2)] {
            let len = fs::metadata(&p).map(|m| m.len()).unwrap_or(0);
            assert!(len <= 100, "{} is {len} bytes", p.display());
            if let Ok(text) = fs::read_to_string(&p) {
                assert!(text.ends_with('\n'));
            }
        }
        assert!(l.error().is_none());
    }

    #[test]
    fn a_log_that_cannot_be_written_keeps_the_reason() {
        let tmp = tempfile::tempdir().unwrap();
        let file = tmp.path().join("not-a-folder");
        fs::write(&file, "x").unwrap();
        let mut l = Logger::new(&file, 1000, 2);
        l.write(Level::Info, "hello");
        let e = l.error().unwrap();
        assert!(e.contains("not-a-folder"), "{e}");
    }

    #[test]
    fn reopening_appends_to_the_same_file() {
        let tmp = tempfile::tempdir().unwrap();
        Logger::new(tmp.path(), 10_000, 2).write(Level::Info, "first run");
        Logger::new(tmp.path(), 10_000, 2).write(Level::Warn, "second run");
        let text = fs::read_to_string(tmp.path().join(NAME)).unwrap();
        assert_eq!(text.lines().count(), 2);
        assert!(text.contains(" WARN  second run"));
    }

    #[test]
    fn masks_secrets_home_and_emails() {
        let home = Some("/home/ola");
        let m = |s: &str| mask(s, home);
        assert_eq!(m("Opened /home/ola/projects/pump"), "Opened ~/projects/pump");
        assert_eq!(m("key sk-ant-api03-abcdefghijklmnop end"), "key sk-ant-*** end");
        assert_eq!(m("gh token ghp_abcdefghijklmnopqrstuvwxyz"), "gh token ghp_***");
        assert_eq!(m("api_key=abcdef123456 next"), "api_key=*** next");
        assert_eq!(m("Authorization: Bearer abcdefghijkl"), "Authorization: Bearer ***");
        assert_eq!(m("password: hunter2hunter2"), "password: ***");
        assert_eq!(m("password: abc"), "password: ***", "short values too");
        assert_eq!(m("password: abcdef!ghijkl end"), "password: *** end", "the whole value");
        assert_eq!(m("token=abc!def end"), "token=*** end");
        assert_eq!(m("from ola.nordmann@example.com today"), "from <email> today");
        assert_eq!(m("two\nlines"), "two lines");
        assert_eq!(m("Saved 3 pages"), "Saved 3 pages", "ordinary text stays");
        assert_eq!(mask(r"C:\Users\ola\x", Some(r"C:\Users\ola")), "~\\x");
    }

    #[test]
    fn timestamps_are_utc_dates() {
        let t = UNIX_EPOCH + std::time::Duration::from_secs(1_794_873_600 + 3 * 3600 + 4 * 60 + 5);
        assert_eq!(timestamp(t), "2026-11-17T03:04:05Z");
        assert_eq!(timestamp(UNIX_EPOCH), "1970-01-01T00:00:00Z");
        assert_eq!(
            timestamp(UNIX_EPOCH + std::time::Duration::from_secs(951_782_400)),
            "2000-02-29T00:00:00Z"
        );
    }
}
