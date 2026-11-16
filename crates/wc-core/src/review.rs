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
}
