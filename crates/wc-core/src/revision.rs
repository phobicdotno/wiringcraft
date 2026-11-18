//! Revisions of a project: the list of issued revisions (letter, date, description, author),
//! clouds drawn on pages around what a revision changed, and a revision table that lists the
//! project's revisions on a page. The editor and the PDF both draw clouds and tables from the
//! shapes made here, so they always match.

use serde::{Deserialize, Serialize};

use crate::geom::{Align, Point, Shape};
use crate::project::Project;
use crate::{Error, Result};

/// The longest revision letter kept, in characters (`A`, `B`, `01`, `P1`, `AA`).
pub const MAX_LETTER: usize = 8;
/// The longest description, date or author kept, in characters.
pub const MAX_FIELD: usize = 200;

/// One issued revision of the project, kept in `project.toml`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Revision {
    /// What clouds and title blocks show: `A`, `B`, `01`, ...; unique in the project.
    pub letter: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub date: String,
    /// What changed.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub text: String,
    /// Who made it.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub author: String,
}

/// A cloud drawn around a change on a page, tagged with the revision that made the change.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RevisionCloud {
    /// Top-left corner of the area the cloud goes around, in millimetres.
    pub min: Point,
    /// Bottom-right corner, in millimetres.
    pub max: Point,
    /// The revision letter in the cloud's tag.
    pub revision: String,
}

/// The length of one scallop of a cloud along its side, in millimetres.
pub const SCALLOP: f64 = 6.0;
/// The side of the triangle that carries a cloud's revision letter, in millimetres.
pub const TAG_SIZE: f64 = 6.0;
/// The text height in a cloud's tag and in a revision table.
const TEXT: f64 = 2.5;

/// The columns of a revision table: heading and width in millimetres.
pub const TABLE_COLUMNS: [(&str, f64); 4] = [("Rev", 12.0), ("Date", 24.0), ("Description", 80.0), ("By", 24.0)];
/// The height of one row of a revision table, the heading row included.
pub const TABLE_ROW: f64 = 5.0;

/// A one-line field as kept: trimmed, without control characters, not overlong.
fn field(s: &str, max: usize) -> String {
    let kept: String = s.chars().filter(|c| !c.is_control()).collect();
    kept.trim().chars().take(max).collect()
}

/// The text height at which `s` fits `width` millimetres: `max` when it fits, smaller when it
/// does not. Revision letters are shrunk rather than cut, so two letters always read apart.
fn fit_height(s: &str, width: f64, max: f64) -> f64 {
    let n = s.chars().count().max(1) as f64;
    // Characters of the PDF font are about 0.6 of the text height wide on average.
    max.min(width / (0.6 * n))
}

/// A cut text that fits `width` millimetres at the table's text height, ending in `...` when cut.
fn fit(s: &str, width: f64) -> String {
    // Characters of the PDF font are about 0.6 of the text height wide on average.
    let room = ((width - 2.0) / (TEXT * 0.6)).floor().max(1.0) as usize;
    if s.chars().count() <= room {
        return s.to_owned();
    }
    let mut out: String = s.chars().take(room.saturating_sub(3)).collect();
    out.push_str("...");
    out
}

impl RevisionCloud {
    /// A cloud around the rectangle with corners `a` and `b` (in any order).
    pub fn new(a: Point, b: Point, revision: impl Into<String>) -> Self {
        Self {
            min: Point::new(a.x.min(b.x), a.y.min(b.y)),
            max: Point::new(a.x.max(b.x), a.y.max(b.y)),
            revision: revision.into(),
        }
    }

    /// Whether the cloud goes around an area: finite corners, some width and height.
    pub fn is_valid(&self) -> bool {
        [self.min.x, self.min.y, self.max.x, self.max.y]
            .iter()
            .all(|v| v.is_finite())
            && self.max.x > self.min.x
            && self.max.y > self.min.y
    }

    /// The cloud's outline: half circles bulging outwards along each side, clockwise on paper
    /// from the top-left corner. Each side gets whole scallops of about [`SCALLOP`] length.
    pub fn scallops(&self) -> Vec<Shape> {
        if !self.is_valid() {
            return Vec::new();
        }
        let (a, b) = (self.min, self.max);
        let corners = [
            Point::new(a.x, a.y),
            Point::new(b.x, a.y),
            Point::new(b.x, b.y),
            Point::new(a.x, b.y),
        ];
        let mut out = Vec::new();
        let next = corners.iter().cycle().skip(1);
        for (&p, &q) in corners.iter().zip(next) {
            let len = p.distance(q);
            // At most 500 scallops a side, so a huge cloud stays cheap to draw.
            let n = ((len / SCALLOP).round() as usize).clamp(1, 500);
            let (dx, dy) = ((q.x - p.x) / n as f64, (q.y - p.y) / n as f64);
            let radius = len / n as f64 / 2.0;
            // Going clockwise on paper, the outside is on the left of the travel direction, and
            // an arc from the start point's angle sweeping 180 degrees (clockwise on paper) bulges
            // to that side.
            let start = dy.atan2(dx).to_degrees() + 180.0;
            for k in 0..n {
                let center = Point::new(p.x + dx * (k as f64 + 0.5), p.y + dy * (k as f64 + 0.5));
                out.push(Shape::Arc {
                    center,
                    radius,
                    start,
                    end: start + 180.0,
                });
            }
        }
        out
    }

    /// The tag: a triangle with the revision letter, standing on the cloud's top-right corner,
    /// just outside the scallops.
    pub fn tag(&self) -> Vec<Shape> {
        if !self.is_valid() {
            return Vec::new();
        }
        let height = TAG_SIZE * 3f64.sqrt() / 2.0;
        // The scallops reach at most half a scallop outside the rectangle.
        let base = self.min.y - SCALLOP / 2.0 - 0.5;
        let mid = self.max.x;
        vec![
            Shape::Polyline {
                points: vec![
                    Point::new(mid - TAG_SIZE / 2.0, base),
                    Point::new(mid + TAG_SIZE / 2.0, base),
                    Point::new(mid, base - height),
                ],
                closed: true,
                filled: false,
            },
            Shape::Text {
                at: Point::new(mid, base - 0.6),
                text: self.revision.clone(),
                height: fit_height(&self.revision, TAG_SIZE * 0.6, 2.0),
                align: Align::Center,
                angle: 0.0,
            },
        ]
    }

    /// Everything the cloud draws: its outline and its tag.
    pub fn shapes(&self) -> Vec<Shape> {
        let mut out = self.scallops();
        out.extend(self.tag());
        out
    }

    /// Whether `at` is on the cloud's outline or its tag, within `tol` millimetres.
    pub fn hit(&self, at: Point, tol: f64) -> bool {
        if !self.is_valid() {
            return false;
        }
        let reach = SCALLOP / 2.0 + tol;
        let inside_outer = at.x >= self.min.x - reach
            && at.x <= self.max.x + reach
            && at.y >= self.min.y - reach
            && at.y <= self.max.y + reach;
        let inside_inner = at.x > self.min.x + reach
            && at.x < self.max.x - reach
            && at.y > self.min.y + reach
            && at.y < self.max.y - reach;
        let height = TAG_SIZE * 3f64.sqrt() / 2.0;
        let base = self.min.y - SCALLOP / 2.0 - 0.5;
        let on_tag =
            (at.x - self.max.x).abs() <= TAG_SIZE / 2.0 + tol && at.y <= base + tol && at.y >= base - height - tol;
        (inside_outer && !inside_inner) || on_tag
    }
}

/// The revision table with its top-left corner at `at`: a heading row and one row per
/// revision, oldest first, as lines and texts.
pub fn table_shapes(revisions: &[Revision], at: Point) -> Vec<Shape> {
    if !(at.x.is_finite() && at.y.is_finite()) {
        return Vec::new();
    }
    let width: f64 = TABLE_COLUMNS.iter().map(|(_, w)| w).sum();
    let rows = revisions.len() + 1;
    let height = TABLE_ROW * rows as f64;
    let mut out = vec![Shape::Rect {
        min: at,
        max: Point::new(at.x + width, at.y + height),
    }];
    for r in 1..rows {
        let y = at.y + TABLE_ROW * r as f64;
        out.push(Shape::Line {
            a: Point::new(at.x, y),
            b: Point::new(at.x + width, y),
        });
    }
    let mut x = at.x;
    for (i, (_, w)) in TABLE_COLUMNS.iter().enumerate() {
        if i > 0 {
            out.push(Shape::Line {
                a: Point::new(x, at.y),
                b: Point::new(x, at.y + height),
            });
        }
        x += w;
    }
    let row_texts = std::iter::once(TABLE_COLUMNS.map(|(h, _)| h.to_owned())).chain(
        revisions
            .iter()
            .map(|r| [r.letter.clone(), r.date.clone(), r.text.clone(), r.author.clone()]),
    );
    for (r, texts) in row_texts.enumerate() {
        let baseline = at.y + TABLE_ROW * (r as f64 + 1.0) - (TABLE_ROW - TEXT) / 2.0 - 0.3;
        let mut x = at.x;
        for (c, (t, (_, w))) in texts.iter().zip(TABLE_COLUMNS).enumerate() {
            if !t.is_empty() {
                // The revision letter is shrunk to fit, never cut; other columns are cut.
                let (text, height) = if c == 0 && r > 0 {
                    (t.clone(), fit_height(t, w - 2.0, TEXT))
                } else {
                    (fit(t, w), TEXT)
                };
                out.push(Shape::Text {
                    at: Point::new(x + 1.0, baseline),
                    text,
                    height,
                    align: Align::Left,
                    angle: 0.0,
                });
            }
            x += w;
        }
    }
    out
}

/// The size of a revision table listing `rows` revisions, in millimetres.
pub fn table_size(rows: usize) -> (f64, f64) {
    (
        TABLE_COLUMNS.iter().map(|(_, w)| w).sum(),
        TABLE_ROW * (rows as f64 + 1.0),
    )
}

impl Project {
    /// The revision with this letter.
    pub fn revision(&self, letter: &str) -> Option<&Revision> {
        self.meta.revisions.iter().find(|r| r.letter == letter.trim())
    }

    /// The latest revision: the last one added.
    pub fn latest_revision(&self) -> Option<&Revision> {
        self.meta.revisions.last()
    }

    /// Add a revision after the others. Refused when the letter is empty or already used.
    pub fn add_revision(&mut self, letter: &str, date: &str, text: &str, author: &str) -> Result<()> {
        let letter = field(letter, MAX_LETTER);
        if letter.is_empty() {
            return Err(Error::Invalid("a revision needs a letter".into()));
        }
        if self.revision(&letter).is_some() {
            return Err(Error::Invalid(format!("revision {letter} is already there")));
        }
        self.meta.revisions.push(Revision {
            letter,
            date: field(date, MAX_FIELD),
            text: field(text, MAX_FIELD),
            author: field(author, MAX_FIELD),
        });
        Ok(())
    }

    /// Change a revision's date, description and author. Returns whether anything changed.
    pub fn edit_revision(&mut self, letter: &str, date: &str, text: &str, author: &str) -> Result<bool> {
        let r = self
            .meta
            .revisions
            .iter_mut()
            .find(|r| r.letter == letter.trim())
            .ok_or_else(|| Error::Invalid(format!("no revision {}", letter.trim())))?;
        let new = Revision {
            letter: r.letter.clone(),
            date: field(date, MAX_FIELD),
            text: field(text, MAX_FIELD),
            author: field(author, MAX_FIELD),
        };
        let changed = *r != new;
        *r = new;
        Ok(changed)
    }

    /// Remove a revision. Refused while a cloud on some page still carries its letter.
    pub fn remove_revision(&mut self, letter: &str) -> Result<()> {
        let letter = letter.trim();
        let i = self
            .meta
            .revisions
            .iter()
            .position(|r| r.letter == letter)
            .ok_or_else(|| Error::Invalid(format!("no revision {letter}")))?;
        if let Some(p) = self
            .pages
            .iter()
            .find(|p| p.clouds.iter().any(|c| c.revision == letter))
        {
            return Err(Error::Invalid(format!(
                "page {} has a cloud of revision {letter}; remove it first",
                p.number
            )));
        }
        self.meta.revisions.remove(i);
        Ok(())
    }

    /// Draw a cloud on a page around the rectangle with corners `a` and `b`, tagged with
    /// `revision`, or with the latest revision when `None`. Returns the cloud's index.
    pub fn add_cloud(&mut self, page: &str, a: Point, b: Point, revision: Option<&str>) -> Result<usize> {
        let letter = match revision {
            Some(l) => self
                .revision(l)
                .ok_or_else(|| Error::Invalid(format!("no revision {}", l.trim())))?
                .letter
                .clone(),
            None => self
                .latest_revision()
                .ok_or_else(|| Error::Invalid("add a revision before drawing a cloud".into()))?
                .letter
                .clone(),
        };
        let cloud = RevisionCloud::new(a, b, letter);
        if !cloud.is_valid() {
            return Err(Error::Invalid(
                "a cloud needs an area with some width and height".into(),
            ));
        }
        let p = self
            .page_mut(page)
            .ok_or_else(|| Error::Invalid(format!("no page {page:?}")))?;
        p.clouds.push(cloud);
        Ok(p.clouds.len() - 1)
    }

    /// Show the revision table on a page with its top-left corner at `at`, or take it off
    /// with `None`. Returns whether anything changed.
    pub fn set_revision_table(&mut self, page: &str, at: Option<Point>) -> Result<bool> {
        if let Some(p) = at
            && !(p.x.is_finite() && p.y.is_finite())
        {
            return Err(Error::Invalid("the table's place must be a finite point".into()));
        }
        let p = self
            .page_mut(page)
            .ok_or_else(|| Error::Invalid(format!("no page {page:?}")))?;
        let changed = p.revision_table != at;
        p.revision_table = at;
        Ok(changed)
    }

    /// The clouds that carry a revision the project does not list, as (page number, letter).
    pub fn clouds_without_revision(&self) -> Vec<(String, String)> {
        self.pages
            .iter()
            .flat_map(|p| {
                p.clouds
                    .iter()
                    .filter(|c| self.revision(&c.revision).is_none())
                    .map(|c| (p.number.clone(), c.revision.clone()))
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::panic, clippy::unreachable)]
    use super::*;
    use crate::project::Page;

    fn project() -> Project {
        let mut p = Project::new("Demo");
        p.pages.push(Page::new("p1", "1", "Power"));
        p
    }

    #[test]
    fn revisions_are_kept_in_order_and_letters_are_unique() {
        let mut p = project();
        p.add_revision(" A ", "2026-11-01", "First issue", "OP").unwrap();
        p.add_revision("B", "2026-11-18", "Motor M2 added", "OP").unwrap();
        assert!(p.add_revision("B", "", "", "").is_err(), "a letter is used once");
        assert!(p.add_revision("  ", "", "", "").is_err(), "a letter is needed");
        assert_eq!(p.latest_revision().unwrap().letter, "B");
        assert_eq!(p.revision("A").unwrap().text, "First issue");
        assert!(p.edit_revision("A", "2026-11-02", "First issue", "OP").unwrap());
        assert!(!p.edit_revision("A", "2026-11-02", "First issue", "OP").unwrap());
        assert!(p.edit_revision("Z", "", "", "").is_err());
    }

    #[test]
    fn a_cloud_takes_the_latest_revision_and_holds_its_revision_in_the_list() {
        let mut p = project();
        assert!(
            p.add_cloud("p1", Point::new(10.0, 10.0), Point::new(40.0, 30.0), None)
                .is_err()
        );
        p.add_revision("A", "", "", "").unwrap();
        p.add_revision("B", "", "", "").unwrap();
        let i = p
            .add_cloud("p1", Point::new(40.0, 30.0), Point::new(10.0, 10.0), None)
            .unwrap();
        let c = &p.page("p1").unwrap().clouds[i];
        assert_eq!(c.revision, "B");
        assert_eq!((c.min, c.max), (Point::new(10.0, 10.0), Point::new(40.0, 30.0)));
        p.add_cloud("p1", Point::new(0.0, 0.0), Point::new(5.0, 5.0), Some("A"))
            .unwrap();
        assert!(
            p.add_cloud("p1", Point::new(0.0, 0.0), Point::new(5.0, 5.0), Some("C"))
                .is_err()
        );
        assert!(
            p.add_cloud("p1", Point::new(0.0, 0.0), Point::new(0.0, 5.0), None)
                .is_err(),
            "no width"
        );
        assert!(
            p.add_cloud("p9", Point::new(0.0, 0.0), Point::new(5.0, 5.0), None)
                .is_err()
        );
        assert!(p.remove_revision("B").is_err(), "a cloud still carries it");
        p.page_mut("p1").unwrap().clouds.remove(0);
        p.remove_revision("B").unwrap();
        assert_eq!(p.latest_revision().unwrap().letter, "A");
    }

    #[test]
    fn scallops_run_around_the_area_and_bulge_outwards() {
        let c = RevisionCloud::new(Point::new(10.0, 20.0), Point::new(40.0, 32.0), "A");
        let arcs = c.scallops();
        // 30 mm sides get 5 scallops, 12 mm sides 2.
        assert_eq!(arcs.len(), 5 + 2 + 5 + 2);
        for a in &arcs {
            let Shape::Arc {
                center,
                radius,
                start,
                end,
            } = a
            else {
                panic!("not an arc: {a:?}");
            };
            assert!((end - start - 180.0).abs() < 1e-9);
            // The middle of the sweep is the arc's farthest point from the area.
            let mid = (start + 90.0).to_radians();
            let top = Point::new(center.x + radius * mid.cos(), center.y + radius * mid.sin());
            let outside = top.x < 10.0 - 1e-9 || top.x > 40.0 + 1e-9 || top.y < 20.0 - 1e-9 || top.y > 32.0 + 1e-9;
            assert!(outside, "the scallop at {center:?} bulges into the area ({top:?})");
        }
        // Neighbouring scallops meet: each ends where the next starts.
        let ends: Vec<(Point, Point)> = arcs
            .iter()
            .map(|a| match a {
                Shape::Arc {
                    center,
                    radius,
                    start,
                    end,
                } => {
                    let at = |d: f64| {
                        let t = d.to_radians();
                        Point::new(center.x + radius * t.cos(), center.y + radius * t.sin())
                    };
                    (at(*start), at(*end))
                }
                _ => unreachable!(),
            })
            .collect();
        for i in 0..ends.len() {
            let next = ends[(i + 1) % ends.len()].0;
            assert!(ends[i].1.distance(next) < 1e-9, "a gap after scallop {i}");
        }
    }

    #[test]
    fn the_tag_stands_above_the_top_right_corner_with_the_letter() {
        let c = RevisionCloud::new(Point::new(10.0, 20.0), Point::new(40.0, 32.0), "B");
        let tag = c.tag();
        assert!(matches!(&tag[1], Shape::Text { text, .. } if text == "B"));
        let Shape::Polyline { points, .. } = &tag[0] else {
            panic!("no triangle")
        };
        assert!(points.iter().all(|p| p.y < 20.0 - SCALLOP / 2.0));
        assert!(points.iter().all(|p| (p.x - 40.0).abs() <= TAG_SIZE / 2.0 + 1e-9));
        assert!(c.hit(Point::new(40.0, 14.0), 0.5), "on the tag");
        assert!(c.hit(Point::new(25.0, 20.0), 0.5), "on the top side");
        assert!(!c.hit(Point::new(25.0, 26.0), 0.5), "inside the area");
        assert!(!c.hit(Point::new(80.0, 26.0), 0.5), "far off");
    }

    #[test]
    fn control_characters_around_a_letter_do_not_hide_it() {
        let mut p = project();
        p.add_revision("\u{1} A \u{1}", "", "", "").unwrap();
        assert_eq!(p.meta.revisions[0].letter, "A");
        assert!(p.add_revision("A", "", "", "").is_err(), "the same letter again");
        assert!(p.edit_revision("A", "x", "", "").unwrap());
        p.remove_revision("A").unwrap();
    }

    #[test]
    fn long_letters_are_shrunk_not_cut() {
        let mut p = project();
        p.add_revision("ABCDEFGH", "", "", "").unwrap();
        p.add_revision("ABCDEFGI", "", "", "").unwrap();
        let texts = |shapes: Vec<Shape>| -> Vec<(String, f64)> {
            shapes
                .into_iter()
                .filter_map(|s| match s {
                    Shape::Text { text, height, .. } => Some((text, height)),
                    _ => None,
                })
                .collect()
        };
        let table = texts(table_shapes(&p.meta.revisions, Point::new(0.0, 0.0)));
        assert!(table.iter().any(|(t, h)| t == "ABCDEFGH" && *h < TEXT));
        assert!(table.iter().any(|(t, _)| t == "ABCDEFGI"));
        for l in ["ABCDEFGH", "ABCDEFGI"] {
            let tag = texts(RevisionCloud::new(Point::new(0.0, 0.0), Point::new(9.0, 9.0), l).tag());
            assert_eq!(tag[0].0, l);
            assert!(tag[0].1 * 0.6 * 8.0 <= TAG_SIZE * 0.6 + 1e-9, "fits the triangle");
        }
    }

    #[test]
    fn an_empty_or_broken_cloud_draws_nothing() {
        let c = RevisionCloud {
            min: Point::new(f64::NAN, 0.0),
            max: Point::new(5.0, 5.0),
            revision: "A".into(),
        };
        assert!(c.shapes().is_empty());
        assert!(!c.hit(Point::new(0.0, 0.0), 1.0));
    }

    #[test]
    fn the_table_lists_every_revision_under_a_heading() {
        let mut p = project();
        p.add_revision("A", "2026-11-01", "First issue", "OP").unwrap();
        p.add_revision("B", "2026-11-18", &"Long text ".repeat(20), "OP")
            .unwrap();
        let shapes = table_shapes(&p.meta.revisions, Point::new(100.0, 50.0));
        let texts: Vec<&str> = shapes
            .iter()
            .filter_map(|s| match s {
                Shape::Text { text, .. } => Some(text.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(&texts[..4], &["Rev", "Date", "Description", "By"]);
        assert_eq!(&texts[4..8], &["A", "2026-11-01", "First issue", "OP"]);
        assert!(texts[10].ends_with("..."), "a long description is cut");
        let (w, h) = table_size(2);
        assert!(matches!(shapes[0], Shape::Rect { max, .. } if max == Point::new(100.0 + w, 50.0 + h)));
        // Every text stays inside the table.
        for s in &shapes {
            if let Shape::Text { at, .. } = s {
                assert!(at.x > 100.0 && at.x < 100.0 + w && at.y > 50.0 && at.y < 50.0 + h);
            }
        }
    }

    #[test]
    fn the_table_is_placed_and_taken_off_a_page() {
        let mut p = project();
        assert!(p.set_revision_table("p1", Some(Point::new(5.0, 5.0))).unwrap());
        assert!(!p.set_revision_table("p1", Some(Point::new(5.0, 5.0))).unwrap());
        assert!(
            p.set_revision_table("p1", Some(Point::new(f64::INFINITY, 5.0)))
                .is_err()
        );
        assert!(p.set_revision_table("p1", None).unwrap());
        assert!(p.set_revision_table("p2", None).is_err());
    }

    #[test]
    fn revisions_clouds_and_tables_survive_a_save() {
        let dir = std::env::temp_dir().join(format!("wc-revision-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let mut p = project();
        p.add_revision("A", "2026-11-01", "First issue", "OP").unwrap();
        p.add_cloud("p1", Point::new(10.0, 10.0), Point::new(40.0, 30.0), None)
            .unwrap();
        p.set_revision_table("p1", Some(Point::new(200.0, 20.0))).unwrap();
        crate::store::save(&dir, &p).unwrap();
        let back = crate::store::load(&dir).unwrap();
        assert_eq!(back.meta.revisions, p.meta.revisions);
        assert_eq!(back.page("p1").unwrap().clouds, p.page("p1").unwrap().clouds);
        assert_eq!(back.page("p1").unwrap().revision_table, Some(Point::new(200.0, 20.0)));
        assert!(back.clouds_without_revision().is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
