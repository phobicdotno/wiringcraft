//! Checks while drawing: the project check runs again shortly after each edit, off the window
//! thread, so its findings can be marked on the canvas and counted in the status bar without
//! slowing typing or dragging.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::sync::mpsc::{Receiver, TryRecvError, channel};
use std::time::{Duration, Instant};

use wc_core::{Part, Project, Symbol};
use wc_report::Finding;

/// How long the project must stay unchanged before it is checked, so a burst of typing or a
/// drag is checked once, after it settles.
pub const SETTLE: Duration = Duration::from_millis(300);

/// How often the window looks for the result of a check that is running.
const POLL: Duration = Duration::from_millis(50);

/// The inputs of one check.
struct Snapshot {
    project: Project,
    symbols: BTreeMap<String, Symbol>,
    parts: BTreeMap<String, Part>,
}

impl Snapshot {
    fn same(&self, project: &Project, symbols: &BTreeMap<String, Symbol>, parts: &BTreeMap<String, Part>) -> bool {
        self.project == *project && self.symbols == *symbols && self.parts == *parts
    }
}

/// The background check and its latest findings.
#[derive(Default)]
pub struct LiveCheck {
    /// The inputs of the latest check started.
    started: Option<Arc<Snapshot>>,
    /// The project differs from `started`; the latest edit to it was at this time. The check
    /// waits until nothing has been edited for [`SETTLE`].
    pending: Option<Instant>,
    /// The check running now.
    job: Option<Receiver<Vec<Finding>>>,
    /// The findings of the latest check that finished.
    pub findings: Vec<Finding>,
    /// Why the latest check could not run or finish, until one does.
    pub failed: Option<String>,
}

/// What [`LiveCheck::poll`] did.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Poll {
    /// The findings changed: show the new ones.
    pub changed: bool,
    /// Look again after this long, even without input.
    pub again: Option<Duration>,
}

impl LiveCheck {
    /// Forget everything: no check runs and there are no findings.
    pub fn clear(&mut self) {
        *self = Self::default();
    }

    /// A check is running or waiting for the project to settle.
    pub fn busy(&self) -> bool {
        self.job.is_some() || self.pending.is_some()
    }

    /// Take a finished check's findings, and start a new check once nothing has been edited
    /// for [`SETTLE`]. `editing` says the user typed, clicked or dragged in this frame: while
    /// the project differs from what was checked, that starts the wait again. Edits that come
    /// without input (the assistant, a shared session) wait from when they were first seen.
    ///
    /// On the window thread this costs one comparison of the inputs with the copy last
    /// checked per frame, and one copy of them per check started, never one per edit. Only one
    /// check runs at a time; an edit made while it runs is checked after it.
    pub fn poll(
        &mut self,
        project: &Project,
        symbols: &BTreeMap<String, Symbol>,
        parts: &BTreeMap<String, Part>,
        editing: bool,
        now: Instant,
    ) -> Poll {
        let mut changed = false;
        if let Some(rx) = &self.job {
            match rx.try_recv() {
                Ok(found) => {
                    changed = found != self.findings || self.failed.is_some();
                    self.findings = found;
                    self.failed = None;
                    self.job = None;
                }
                Err(TryRecvError::Empty) => {
                    // An edit while the check runs: its wait starts at the latest edit.
                    if editing {
                        self.pending = Some(now);
                    }
                    return Poll {
                        changed,
                        again: Some(POLL),
                    };
                }
                // The check panicked: say so and try again on the next edit.
                Err(TryRecvError::Disconnected) => {
                    self.job = None;
                    self.findings.clear();
                    self.failed = Some("the check stopped before it finished".into());
                    changed = true;
                }
            }
        }
        if self.started.as_ref().is_some_and(|s| s.same(project, symbols, parts)) {
            self.pending = None;
            return Poll { changed, again: None };
        }
        let since = match self.pending {
            Some(t) if !editing => t,
            _ => *self.pending.insert(now),
        };
        let waited = now.saturating_duration_since(since);
        if waited < SETTLE {
            return Poll {
                changed,
                again: Some(SETTLE - waited),
            };
        }
        self.pending = None;
        let snap = Arc::new(Snapshot {
            project: project.clone(),
            symbols: symbols.clone(),
            parts: parts.clone(),
        });
        let (tx, rx) = channel();
        let input = Arc::clone(&snap);
        let spawned = std::thread::Builder::new().name("live-check".into()).spawn(move || {
            let found = wc_report::check(&wc_report::Inputs {
                project: &input.project,
                symbols: &input.symbols,
                parts: &input.parts,
            });
            // The window may have moved on; then nobody waits for this.
            let _ = tx.send(found);
        });
        self.started = Some(snap);
        match spawned {
            Ok(_) => {
                self.job = Some(rx);
                Poll {
                    changed,
                    again: Some(POLL),
                }
            }
            // No thread to be had: say why (the old findings may be for other inputs), and
            // try again on the next edit.
            Err(e) => {
                self.findings.clear();
                self.failed = Some(format!("the check could not be started: {e}"));
                Poll {
                    changed: true,
                    again: None,
                }
            }
        }
    }

    /// The findings to show for these inputs: the latest check's when it was of exactly
    /// them and no newer edit waits; `None` while a check is pending, when the findings may be
    /// of other inputs (another project, say).
    pub fn settled(&self) -> Option<&[Finding]> {
        (!self.busy()).then_some(self.findings.as_slice())
    }

    /// The status bar text: errors and warnings, that all is well, or why the check failed.
    pub fn status(&self) -> String {
        match &self.failed {
            Some(why) => format!("Check: not run, {why}"),
            None => summary(&self.findings),
        }
    }
}

/// The status bar text for a set of findings: errors and warnings, or that all is well.
pub fn summary(found: &[Finding]) -> String {
    use wc_report::Severity;
    let errors = found.iter().filter(|f| f.severity == Severity::Error).count();
    let warnings = found.iter().filter(|f| f.severity == Severity::Warning).count();
    let plural = |n: usize, one: &str| {
        if n == 1 {
            format!("1 {one}")
        } else {
            format!("{n} {one}s")
        }
    };
    match (errors, warnings) {
        (0, 0) => "Check: no problems".into(),
        (0, w) => format!("Check: {}", plural(w, "warning")),
        (e, 0) => format!("Check: {}", plural(e, "error")),
        (e, w) => format!("Check: {}, {}", plural(e, "error"), plural(w, "warning")),
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use crate::state::Open;
    use wc_core::DeviceTag;
    use wc_core::geom::Point;
    use wc_report::Severity;

    /// Poll until the running check is done (it is quick on a small project).
    fn settle(live: &mut LiveCheck, o: &Open, now: &mut Instant) -> bool {
        let mut changed = false;
        for _ in 0..1000 {
            let p = live.poll(&o.project, &o.symbols, &o.parts, false, *now);
            changed |= p.changed;
            if p.again.is_none() {
                return changed;
            }
            *now += POLL;
            std::thread::sleep(Duration::from_millis(5));
        }
        unreachable!("the check did not finish");
    }

    /// A project with two coils both tagged -K1 on page 2.
    fn coil_twice() -> (tempfile::TempDir, Open) {
        let (tmp, mut o) = crate::state::tests::project();
        for x in [20.0, 60.0] {
            let id = o.place(1, "wc.coil", Point::new(x, 20.0)).unwrap();
            let page = &mut o.project.pages[1];
            let s = page.symbols.iter_mut().find(|s| s.id == id).unwrap();
            s.tag = Some(DeviceTag::parse("-K1").unwrap());
        }
        (tmp, o)
    }

    fn rename_second_coil(o: &mut Open, tag: &str) {
        let s = o.project.pages[1].symbols.last_mut().unwrap();
        s.tag = Some(DeviceTag::parse(tag).unwrap());
    }

    fn duplicate_tag_errors(f: &[Finding]) -> usize {
        f.iter()
            .filter(|f| f.severity == Severity::Error && f.message.contains("-K1 is drawn"))
            .count()
    }

    #[test]
    fn a_duplicate_tag_is_marked_after_the_edit_settles_and_clears_when_fixed() {
        let (_dir, mut o) = coil_twice();
        let mut live = LiveCheck::default();
        let mut now = Instant::now();
        settle(&mut live, &o, &mut now);
        assert_eq!(
            duplicate_tag_errors(&live.findings),
            2,
            "one on each coil: {:?}",
            live.findings
        );

        // Rename the second coil: nothing is checked until the edit has settled.
        rename_second_coil(&mut o, "-K2");
        let p = live.poll(&o.project, &o.symbols, &o.parts, true, now);
        assert!(!p.changed);
        assert!(p.again.is_some_and(|d| d <= SETTLE), "{p:?}");
        assert!(live.busy());
        assert_eq!(
            duplicate_tag_errors(&live.findings),
            2,
            "the old findings stay until the check runs"
        );

        now += SETTLE;
        assert!(settle(&mut live, &o, &mut now), "the findings changed");
        assert_eq!(duplicate_tag_errors(&live.findings), 0, "{:?}", live.findings);
        assert!(!live.busy());

        // Unchanged: no new check is started.
        let p = live.poll(&o.project, &o.symbols, &o.parts, true, now);
        assert_eq!(p, Poll::default());
    }

    #[test]
    fn edits_in_a_burst_restart_nothing_until_they_stop() {
        let (_dir, mut o) = coil_twice();
        let mut live = LiveCheck::default();
        let mut now = Instant::now();
        settle(&mut live, &o, &mut now);
        // Keystrokes 100 ms apart: each one only pushes the check back.
        for tag in ["-K", "-K2", "-K22"] {
            rename_second_coil(&mut o, tag);
            let p = live.poll(&o.project, &o.symbols, &o.parts, true, now);
            assert!(p.again.is_some());
            now += Duration::from_millis(100);
        }
        assert!(live.job.is_none(), "no check ran while typing");
        // 300 ms after the first keystroke but only 100 ms after the last: still waiting.
        let p = live.poll(&o.project, &o.symbols, &o.parts, false, now);
        assert!(p.again.is_some_and(|d| d > Duration::ZERO), "{p:?}");
        assert!(live.job.is_none(), "the wait starts again at each edit");
        assert!(live.settled().is_none(), "the findings are not of these inputs yet");
        now += SETTLE;
        settle(&mut live, &o, &mut now);
        assert_eq!(duplicate_tag_errors(&live.findings), 0);
        assert_eq!(live.settled().map(<[Finding]>::len), Some(live.findings.len()));
    }

    #[test]
    fn an_edit_without_input_waits_from_when_it_was_first_seen() {
        let (_dir, mut o) = coil_twice();
        let mut live = LiveCheck::default();
        let mut now = Instant::now();
        settle(&mut live, &o, &mut now);
        // The assistant renames the coil: no keystroke, so later frames do not push it back.
        rename_second_coil(&mut o, "-K2");
        for _ in 0..3 {
            assert!(live.poll(&o.project, &o.symbols, &o.parts, false, now).again.is_some());
            now += Duration::from_millis(100);
        }
        live.poll(&o.project, &o.symbols, &o.parts, false, now);
        assert!(live.job.is_some(), "the check started 300 ms after the edit was seen");
    }

    #[test]
    fn findings_of_another_project_are_not_settled_for_this_one() {
        let (_a, a) = coil_twice();
        let (_b, b) = crate::state::tests::project();
        let mut live = LiveCheck::default();
        let mut now = Instant::now();
        settle(&mut live, &a, &mut now);
        assert!(duplicate_tag_errors(live.settled().unwrap()) > 0);
        // Project B is opened: until its own check is done, A's findings are not shown for it.
        live.poll(&b.project, &b.symbols, &b.parts, false, now);
        assert!(live.settled().is_none());
        now += SETTLE;
        settle(&mut live, &b, &mut now);
        assert_eq!(duplicate_tag_errors(live.settled().unwrap()), 0);
    }

    #[test]
    fn the_summary_counts_errors_and_warnings() {
        let f = |severity| Finding {
            severity,
            page_id: None,
            item: None,
            message: String::new(),
        };
        assert_eq!(summary(&[]), "Check: no problems");
        assert_eq!(summary(&[f(Severity::Note)]), "Check: no problems");
        assert_eq!(summary(&[f(Severity::Error)]), "Check: 1 error");
        assert_eq!(
            summary(&[f(Severity::Error), f(Severity::Error), f(Severity::Warning)]),
            "Check: 2 errors, 1 warning"
        );
        assert_eq!(
            summary(&[f(Severity::Warning), f(Severity::Warning)]),
            "Check: 2 warnings"
        );

        let mut live = LiveCheck {
            findings: vec![f(Severity::Error)],
            ..LiveCheck::default()
        };
        assert_eq!(live.status(), "Check: 1 error");
        live.failed = Some("the check could not be started: no threads".into());
        assert_eq!(
            live.status(),
            "Check: not run, the check could not be started: no threads",
            "a failure is not shown as a clean result"
        );
    }
}
