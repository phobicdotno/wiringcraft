//! The subway map from the outside: lanes laid out for shapes of history the crate's own tests
//! do not draw (octopus merges, lanes freed and reused, a window that cuts parents off), the
//! terminal drawing's glyphs, and `history` on a real repository with tags and a detached HEAD.

use std::path::Path;
use std::process::Command;

use wc_vcs::subway::layout;
use wc_vcs::{Commit, Edge, Git, history, subway_svg, subway_text};

type TestResult = std::result::Result<(), Box<dyn std::error::Error>>;

fn c(id: &str, parents: &[&str]) -> Commit {
    Commit {
        id: id.into(),
        parents: parents.iter().map(|p| p.to_string()).collect(),
        refs: vec![],
        head: false,
        subject: format!("subject {id}"),
        author: "a".into(),
        time: 0,
    }
}

/// Plain git in `dir`, for what [`Git`] has no call for (tags, a detached HEAD).
fn git(dir: &Path, args: &[&str]) -> std::result::Result<String, Box<dyn std::error::Error>> {
    let out = Command::new("git")
        .args(args)
        .current_dir(dir)
        .env_remove("GIT_DIR")
        .env_remove("GIT_WORK_TREE")
        .env_remove("GIT_INDEX_FILE")
        .output()?;
    if !out.status.success() {
        return Err(format!("git {args:?}: {}", String::from_utf8_lossy(&out.stderr)).into());
    }
    Ok(String::from_utf8(out.stdout)?)
}

#[test]
fn an_empty_history_draws_an_empty_map() {
    let rows = layout(vec![]);
    assert!(rows.is_empty());
    assert_eq!(subway_text(&rows), "");
    let svg = subway_svg(&rows);
    assert!(svg.starts_with("<svg") && svg.ends_with("</svg>\n"), "{svg}");
    assert!(!svg.contains("<circle"), "{svg}");
}

#[test]
fn an_octopus_merge_joins_each_parent_from_a_lane_of_its_own() {
    // o merges a, b and d, which all grow from the root r.
    let rows = layout(vec![
        c("o", &["a", "b", "d"]),
        c("a", &["r"]),
        c("b", &["r"]),
        c("d", &["r"]),
        c("r", &[]),
    ]);
    let Some(o) = rows.first() else { unreachable!() };
    assert_eq!(o.lane, 0);
    assert_eq!(
        o.edges,
        vec![
            Edge { to_row: 1, lane: 0 },
            Edge { to_row: 2, lane: 1 },
            Edge { to_row: 3, lane: 2 }
        ]
    );
    let lanes: Vec<usize> = rows.iter().map(|r| r.lane).collect();
    assert_eq!(
        lanes,
        vec![0, 0, 1, 2, 0],
        "every line ends at the root in the first lane"
    );
    // Below the merge, the two side lines pass through the first parent's row.
    assert_eq!(rows.get(1).map(|r| r.through.clone()), Some(vec![1, 2]));
    // The root is where all three lines stop: nothing passes it.
    assert_eq!(rows.get(4).map(|r| r.through.clone()), Some(vec![]));
    let text = subway_text(&rows);
    assert_eq!(text.lines().next(), Some("◆      o subject o"), "{text}");
}

#[test]
fn a_lane_freed_by_a_finished_branch_is_used_again() {
    // Two side branches merged one after the other: the second takes the lane the first left.
    let rows = layout(vec![
        c("m2", &["m1", "y"]),
        c("y", &["m1"]),
        c("m1", &["base", "x"]),
        c("x", &["base"]),
        c("base", &[]),
    ]);
    let lanes: Vec<usize> = rows.iter().map(|r| r.lane).collect();
    assert_eq!(lanes, vec![0, 1, 0, 1, 0]);
    let widest = rows
        .iter()
        .flat_map(|r| r.edges.iter().map(|e| e.lane).chain([r.lane]))
        .max();
    assert_eq!(widest, Some(1), "two lanes are enough");
    // Every line of the text drawing is equally wide: two lanes.
    for line in subway_text(&rows).lines() {
        assert!(line.starts_with(['◆', '●', '│']), "{line:?}");
        assert_eq!(line.chars().nth(4), Some(' '), "{line:?}");
    }
}

#[test]
fn parents_outside_the_window_get_no_line() {
    // Only the newest two commits were read: b's parent and a's parents are not among them.
    let rows = layout(vec![c("b", &["a"]), c("a", &["older", "other"])]);
    let (Some(b), Some(a)) = (rows.first(), rows.get(1)) else {
        unreachable!()
    };
    assert_eq!(b.edges, vec![Edge { to_row: 1, lane: 0 }]);
    assert!(a.edges.is_empty(), "{:?}", a.edges);
    assert_eq!(a.lane, 0);
    assert!(a.through.is_empty());
    // A merge shows as a merge even when its parents are cut off.
    assert!(subway_text(&rows).lines().nth(1).is_some_and(|l| l.starts_with('◆')));
    assert_eq!(subway_svg(&rows).matches("<path").count(), 1);
}

#[test]
fn the_text_map_marks_head_merges_lines_passing_and_refs() {
    let mut m = c("0123456789abcdef", &["a2", "b1"]);
    m.head = true;
    m.refs = vec!["main".into(), "#v2".into()];
    let mut b1 = c("b1", &["a1"]);
    b1.refs = vec!["feature".into()];
    let rows = layout(vec![m, c("a2", &["a1"]), b1, c("a1", &[])]);
    let text = subway_text(&rows);
    let lines: Vec<&str> = text.lines().collect();
    assert_eq!(
        lines,
        vec![
            // The current commit wins over the merge mark; ids are cut to seven characters.
            "◉    0123456 [main, #v2] subject 0123456789abcdef",
            // The main line passes the side branch's commit, and the side line passes a2.
            "● │  a2 subject a2",
            "│ ●  b1 [feature] subject b1",
            "●    a1 subject a1",
        ],
        "{text}"
    );
}

#[test]
fn history_reads_tags_authors_and_a_detached_head() -> TestResult {
    let tmp = tempfile::tempdir()?;
    let dir = tmp.path();
    let g = Git::new(dir);
    g.init()?;
    std::fs::write(dir.join("a.txt"), "one\n")?;
    let first = g
        .commit_as("First change", "Ann Example <ann@example.com>")?
        .ok_or("nothing recorded")?;
    git(dir, &["tag", "v1"])?;
    std::fs::write(dir.join("a.txt"), "two\n")?;
    g.commit("Second change")?.ok_or("nothing recorded")?;

    let rows = history(&g, 10)?;
    let subjects: Vec<&str> = rows.iter().map(|r| r.commit.subject.as_str()).collect();
    assert_eq!(subjects, vec!["Second change", "First change", "New project"]);
    let Some(tagged) = rows.iter().find(|r| r.commit.id == first) else {
        unreachable!("{rows:?}")
    };
    assert_eq!(tagged.commit.refs, vec!["#v1"]);
    assert_eq!(tagged.commit.author, "Ann Example");
    assert!(!tagged.commit.head);
    assert!(tagged.commit.time > 0);
    assert!(rows.first().is_some_and(|r| r.commit.head && r.commit.refs == ["main"]));
    assert!(rows.iter().all(|r| r.lane == 0), "one straight line");

    // Detached at the tagged version: it is the current one, and `main` is just a branch.
    git(dir, &["checkout", "-q", "--detach", "v1"])?;
    let rows = history(&g, 10)?;
    let Some(tagged) = rows.iter().find(|r| r.commit.id == first) else {
        unreachable!("{rows:?}")
    };
    assert!(tagged.commit.head);
    assert_eq!(tagged.commit.refs, vec!["#v1"], "a bare HEAD is not a ref");
    assert!(
        rows.first()
            .is_some_and(|r| !r.commit.head && r.commit.refs == ["main"])
    );
    Ok(())
}

#[test]
fn history_stops_at_the_limit() -> TestResult {
    let tmp = tempfile::tempdir()?;
    let dir = tmp.path();
    let g = Git::new(dir);
    g.init()?;
    for i in 0..3 {
        std::fs::write(dir.join("a.txt"), format!("{i}\n"))?;
        g.commit(&format!("Change {i}"))?;
    }
    let rows = history(&g, 2)?;
    let subjects: Vec<&str> = rows.iter().map(|r| r.commit.subject.as_str()).collect();
    assert_eq!(subjects, vec!["Change 2", "Change 1"]);
    // The oldest row read still knows its parent, but has no line to it.
    let Some(last) = rows.last() else { unreachable!() };
    assert_eq!(last.commit.parents.len(), 1);
    assert!(last.edges.is_empty());
    Ok(())
}
