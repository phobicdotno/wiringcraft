//! Review comments on pages: a note pinned to a spot on a page, with replies, that is open
//! until someone resolves it. Comments are kept per page in `comments/<page id>.comments.json`,
//! so several people can review different pages and merge their work in git without conflicts.

use serde::{Deserialize, Serialize};

use crate::geom::Point;
use crate::project::Project;
use crate::{Error, Result};

/// The longest comment or reply text kept, in characters.
pub const MAX_TEXT: usize = 4000;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Comment {
    /// Unique in the project: `c1`, `c2`, ...
    pub id: String,
    /// The page id it is on.
    pub page: String,
    /// Where on the page it is pinned, in mm; none for the page as a whole.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub at: Option<Point>,
    pub author: String,
    /// Unix seconds.
    pub time: i64,
    pub text: String,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub resolved: bool,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub replies: Vec<Reply>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Reply {
    pub author: String,
    /// Unix seconds.
    pub time: i64,
    pub text: String,
}

/// Unix seconds as a UTC date and time, `YYYY-MM-DD hh:mm`.
pub fn utc_date(secs: i64) -> String {
    let (days, rest) = (secs.div_euclid(86_400), secs.rem_euclid(86_400));
    // Civil date from days since 1970-01-01 (Howard Hinnant's algorithm).
    let z = days.saturating_add(719_468);
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe.saturating_add(era.saturating_mul(400)) + i64::from(month <= 2);
    format!("{year:04}-{month:02}-{day:02} {:02}:{:02}", rest / 3600, rest / 60 % 60)
}

/// Comment text as kept: trimmed, not empty, not overlong.
fn clean(text: &str) -> Result<String> {
    let t = text.trim();
    if t.is_empty() {
        return Err(Error::Invalid("a comment needs some text".into()));
    }
    Ok(t.chars().take(MAX_TEXT).collect())
}

/// An author name as kept: trimmed, one line, `someone` when empty.
fn author(name: &str) -> String {
    let a: String = name.trim().chars().filter(|c| !c.is_control()).take(80).collect();
    if a.is_empty() { "someone".into() } else { a }
}

impl Project {
    /// Pin a new comment on a page. Returns its id.
    pub fn add_comment(&mut self, page: &str, at: Option<Point>, who: &str, time: i64, text: &str) -> Result<String> {
        if self.page(page).is_none() {
            return Err(Error::Invalid(format!("no page {page:?}")));
        }
        if let Some(p) = at
            && !(p.x.is_finite() && p.y.is_finite())
        {
            return Err(Error::Invalid("a comment's position must be a number".into()));
        }
        let text = clean(text)?;
        let next = self
            .comments
            .iter()
            .filter_map(|c| c.id.strip_prefix('c').and_then(|n| n.parse::<u64>().ok()))
            .max()
            .unwrap_or(0)
            .saturating_add(1);
        let id = format!("c{next}");
        self.comments.push(Comment {
            id: id.clone(),
            page: page.into(),
            at,
            author: author(who),
            time,
            text,
            resolved: false,
            replies: Vec::new(),
        });
        Ok(id)
    }

    pub fn comment_mut(&mut self, id: &str) -> Result<&mut Comment> {
        self.comments
            .iter_mut()
            .find(|c| c.id == id)
            .ok_or_else(|| Error::Invalid(format!("no comment {id:?}")))
    }

    /// Answer a comment. Replying to a resolved comment opens it again.
    pub fn reply_to_comment(&mut self, id: &str, who: &str, time: i64, text: &str) -> Result<()> {
        let text = clean(text)?;
        let c = self.comment_mut(id)?;
        c.replies.push(Reply {
            author: author(who),
            time,
            text,
        });
        c.resolved = false;
        Ok(())
    }

    /// Mark a comment resolved, or open it again.
    pub fn resolve_comment(&mut self, id: &str, resolved: bool) -> Result<()> {
        self.comment_mut(id)?.resolved = resolved;
        Ok(())
    }

    pub fn remove_comment(&mut self, id: &str) -> Result<Comment> {
        let i = self
            .comments
            .iter()
            .position(|c| c.id == id)
            .ok_or_else(|| Error::Invalid(format!("no comment {id:?}")))?;
        Ok(self.comments.remove(i))
    }

    /// The comments on one page, oldest first.
    pub fn page_comments(&self, page: &str) -> impl Iterator<Item = &Comment> {
        self.comments.iter().filter(move |c| c.page == page)
    }

    /// Put comments in a fixed order: by page, then by number.
    pub fn sort_comments(&mut self) {
        let num = |id: &str| {
            id.strip_prefix('c')
                .and_then(|n| n.parse::<u64>().ok())
                .unwrap_or(u64::MAX)
        };
        self.comments
            .sort_by(|a, b| (&a.page, num(&a.id), &a.id).cmp(&(&b.page, num(&b.id), &b.id)));
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;
    use crate::Page;

    #[test]
    fn comments_are_numbered_answered_and_resolved() {
        let mut p = Project::new("t");
        p.pages.push(Page::new("p1", "1", "Power"));
        let a = p
            .add_comment(
                "p1",
                Some(Point::new(10.0, 20.0)),
                " Kari ",
                100,
                "  Check the fuse size  ",
            )
            .unwrap();
        let b = p.add_comment("p1", None, "", 101, "Title is wrong").unwrap();
        assert_eq!((a.as_str(), b.as_str()), ("c1", "c2"));
        assert_eq!(p.comments[0].text, "Check the fuse size");
        assert_eq!(p.comments[0].author, "Kari");
        assert_eq!(p.comments[1].author, "someone");
        assert!(p.add_comment("nope", None, "x", 1, "hi").is_err());
        assert!(p.add_comment("p1", None, "x", 1, "   ").is_err());
        assert!(
            p.add_comment("p1", Some(Point::new(f64::NAN, 0.0)), "x", 1, "hi")
                .is_err()
        );

        p.resolve_comment("c1", true).unwrap();
        assert!(p.comments[0].resolved);
        p.reply_to_comment("c1", "Ola", 200, "16 A is right").unwrap();
        assert!(!p.comments[0].resolved, "a reply opens it again");
        assert_eq!(p.comments[0].replies[0].text, "16 A is right");
        assert!(p.reply_to_comment("c9", "Ola", 200, "x").is_err());

        // A removed number is not given out again while a higher one exists.
        p.remove_comment("c1").unwrap();
        assert_eq!(p.add_comment("p1", None, "x", 1, "again").unwrap(), "c3");
        assert_eq!(p.page_comments("p1").count(), 2);
        assert_eq!("x".repeat(MAX_TEXT + 10).len(), MAX_TEXT + 10);
        let long = p.add_comment("p1", None, "x", 1, &"x".repeat(MAX_TEXT + 10)).unwrap();
        assert_eq!(p.comment_mut(&long).unwrap().text.chars().count(), MAX_TEXT);
    }

    #[test]
    fn dates() {
        assert_eq!(utc_date(0), "1970-01-01 00:00");
        assert_eq!(utc_date(1_794_870_807), "2026-11-16 23:13");
        assert_eq!(utc_date(951_782_400), "2000-02-29 00:00");
        // Hostile times never panic.
        let _ = utc_date(i64::MAX);
        let _ = utc_date(i64::MIN);
    }
}

#[cfg(test)]
mod edge_tests {
    use super::*;
    use crate::Page;

    fn project() -> Project {
        let mut p = Project::new("t");
        p.pages.push(Page::new("p1", "1", "Power"));
        p.pages.push(Page::new("p2", "2", "Control"));
        p
    }

    fn comment(id: &str, page: &str) -> Comment {
        Comment {
            id: id.into(),
            page: page.into(),
            at: None,
            author: "a".into(),
            time: 0,
            text: "t".into(),
            resolved: false,
            replies: Vec::new(),
        }
    }

    #[test]
    fn sorting_goes_by_page_then_number_with_odd_ids_last() {
        let mut p = project();
        p.comments = ["c10", "cx", "c2", "c1"].iter().map(|id| comment(id, "p2")).collect();
        p.comments.push(comment("c3", "p1"));
        p.sort_comments();
        let order: Vec<(&str, &str)> = p.comments.iter().map(|c| (c.page.as_str(), c.id.as_str())).collect();
        assert_eq!(
            order,
            [("p1", "c3"), ("p2", "c1"), ("p2", "c2"), ("p2", "c10"), ("p2", "cx")]
        );
    }

    #[test]
    fn numbering_skips_ids_that_are_not_numbers_and_counts_across_pages() {
        let mut p = project();
        p.comments = vec![comment("cx", "p1"), comment("note", "p1"), comment("c7", "p2")];
        let Ok(id) = p.add_comment("p1", None, "x", 1, "hi") else {
            unreachable!()
        };
        assert_eq!(id, "c8");
        assert_eq!(p.page_comments("p1").count(), 3);
        assert_eq!(p.page_comments("p2").count(), 1);
        assert_eq!(p.page_comments("p3").count(), 0);
    }

    #[test]
    fn authors_lose_control_characters_and_are_cut_to_80_characters() {
        let mut p = project();
        let Ok(a) = p.add_comment("p1", None, "Ka\nri\t\u{7}", 1, "hi") else {
            unreachable!()
        };
        let long = "ø".repeat(100);
        let Ok(b) = p.add_comment("p1", None, &long, 1, "hi") else {
            unreachable!()
        };
        let Ok(c) = p.add_comment("p1", None, "\n\t", 1, "hi") else {
            unreachable!()
        };
        let author = |p: &mut Project, id: &str| match p.comment_mut(id) {
            Ok(c) => c.author.clone(),
            Err(_) => unreachable!(),
        };
        assert_eq!(author(&mut p, &a), "Kari");
        assert_eq!(author(&mut p, &b), "ø".repeat(80));
        assert_eq!(author(&mut p, &c), "someone");
    }

    #[test]
    fn long_text_is_cut_by_characters_not_bytes() {
        let mut p = project();
        let text = "æ".repeat(MAX_TEXT + 1);
        let Ok(id) = p.add_comment("p1", Some(Point::new(1.0, 2.0)), "x", 1, &text) else {
            unreachable!()
        };
        let Ok(c) = p.comment_mut(&id) else { unreachable!() };
        assert_eq!(c.text, "æ".repeat(MAX_TEXT));
    }

    #[test]
    fn an_infinite_position_is_refused_and_nothing_is_added() {
        let mut p = project();
        assert!(
            p.add_comment("p1", Some(Point::new(0.0, f64::INFINITY)), "x", 1, "hi")
                .is_err()
        );
        assert!(p.comments.is_empty());
    }

    #[test]
    fn an_empty_reply_is_refused_and_leaves_the_comment_alone() {
        let mut p = project();
        let Ok(id) = p.add_comment("p1", None, "x", 1, "hi") else {
            unreachable!()
        };
        assert!(p.resolve_comment(&id, true).is_ok());
        assert!(p.reply_to_comment(&id, "y", 2, " \n ").is_err());
        let Ok(c) = p.comment_mut(&id) else { unreachable!() };
        assert!(c.resolved, "a refused reply does not open the comment");
        assert!(c.replies.is_empty());
        assert!(p.resolve_comment(&id, false).is_ok());
        let Ok(c) = p.comment_mut(&id) else { unreachable!() };
        assert!(!c.resolved);
    }

    #[test]
    fn unknown_ids_are_refused_by_resolve_remove_and_lookup() {
        let mut p = project();
        assert!(matches!(p.resolve_comment("c1", true), Err(Error::Invalid(_))));
        assert!(matches!(p.remove_comment("c1"), Err(Error::Invalid(_))));
        assert!(matches!(p.comment_mut("c1"), Err(Error::Invalid(_))));
        let Ok(id) = p.add_comment("p2", None, "x", 1, "hi") else {
            unreachable!()
        };
        let Ok(gone) = p.remove_comment(&id) else {
            unreachable!()
        };
        assert_eq!((gone.id.as_str(), gone.page.as_str()), ("c1", "p2"));
        assert!(p.remove_comment(&id).is_err(), "removed only once");
    }

    #[test]
    fn json_leaves_out_defaults_and_reads_them_back() {
        let c = comment("c1", "p1");
        let Ok(text) = serde_json::to_string(&c) else {
            unreachable!()
        };
        assert!(
            !text.contains("resolved") && !text.contains("replies") && !text.contains("\"at\""),
            "{text}"
        );
        let Ok(back) = serde_json::from_str::<Comment>(&text) else {
            unreachable!()
        };
        assert_eq!(back, c);
        let full = Comment {
            at: Some(Point::new(1.5, -2.0)),
            resolved: true,
            replies: vec![Reply {
                author: "b".into(),
                time: 5,
                text: "ok".into(),
            }],
            ..c
        };
        let Ok(text) = serde_json::to_string(&full) else {
            unreachable!()
        };
        let Ok(back) = serde_json::from_str::<Comment>(&text) else {
            unreachable!()
        };
        assert_eq!(back, full);
    }

    #[test]
    fn dates_before_1970_and_within_a_day() {
        assert_eq!(utc_date(-1), "1969-12-31 23:59");
        assert_eq!(utc_date(-86_400), "1969-12-31 00:00");
        assert_eq!(utc_date(86_399), "1970-01-01 23:59");
        assert_eq!(utc_date(3_661), "1970-01-01 01:01");
        assert_eq!(utc_date(4_107_542_400), "2100-03-01 00:00");
    }
}
