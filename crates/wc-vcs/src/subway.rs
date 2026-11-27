//! The commit graph as a subway map: each branch is a coloured line, commits are stations,
//! branch-offs and merges are where lines split and join.

use std::fmt::Write as _;

use crate::{Git, Result};

#[derive(Debug, Clone, PartialEq)]
pub struct Commit {
    pub id: String,
    pub parents: Vec<String>,
    /// Branch and tag names pointing here (`HEAD -> main` is reported as `main`, with `head`).
    pub refs: Vec<String>,
    pub head: bool,
    pub subject: String,
    pub author: String,
    /// Unix seconds.
    pub time: i64,
}

/// A line from one row's station to a parent station further down.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Edge {
    pub to_row: usize,
    /// The lane the line travels in between the two rows.
    pub lane: usize,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Row {
    pub commit: Commit,
    pub lane: usize,
    pub edges: Vec<Edge>,
    /// Lanes with a line passing through this row (not stopping here).
    pub through: Vec<usize>,
}

fn parse_log(text: &str) -> Vec<Commit> {
    crate::log_records(text)
        .into_iter()
        .map(|[id, parents, refs, subject, author, time]| {
            let mut head = false;
            let refs = refs
                .split(", ")
                .filter(|r| !r.is_empty())
                .filter_map(|r| {
                    if let Some(b) = r.strip_prefix("HEAD -> ") {
                        head = true;
                        Some(b.to_owned())
                    } else if r == "HEAD" {
                        head = true;
                        None
                    } else {
                        Some(
                            r.strip_prefix("tag: ")
                                .map(|t| format!("#{t}"))
                                .unwrap_or_else(|| r.to_owned()),
                        )
                    }
                })
                .collect();
            Commit {
                id: id.to_owned(),
                parents: parents
                    .split(' ')
                    .filter(|p| !p.is_empty())
                    .map(str::to_owned)
                    .collect(),
                refs,
                head,
                subject: subject.to_owned(),
                author: author.to_owned(),
                time: time.trim().parse().unwrap_or(0),
            }
        })
        .collect()
}

/// Assign lanes, newest first. Classic "rails" layout: a commit takes the lane that was waiting
/// for it (or a free one); its first parent continues in that lane; other parents get lanes of
/// their own, so merges show up as a line joining from the side.
pub fn layout(commits: Vec<Commit>) -> Vec<Row> {
    let index: std::collections::HashMap<String, usize> =
        commits.iter().enumerate().map(|(i, c)| (c.id.clone(), i)).collect();
    let mut lanes: Vec<Option<String>> = Vec::new();
    let mut rows = Vec::with_capacity(commits.len());
    for c in commits {
        let waiting: Vec<usize> = lanes
            .iter()
            .enumerate()
            .filter(|(_, l)| l.as_deref() == Some(c.id.as_str()))
            .map(|(i, _)| i)
            .collect();
        let lane = match waiting.first() {
            Some(l) => *l,
            None => match lanes.iter().position(Option::is_none) {
                Some(free) => free,
                None => {
                    lanes.push(None);
                    lanes.len() - 1
                }
            },
        };
        for l in &waiting {
            lanes[*l] = None;
        }
        let through: Vec<usize> = lanes
            .iter()
            .enumerate()
            .filter(|(_, l)| l.is_some())
            .map(|(i, _)| i)
            .collect();
        let mut edges = Vec::new();
        for (k, p) in c.parents.iter().enumerate() {
            let Some(&to_row) = index.get(p) else { continue };
            let travel = if k == 0 {
                lanes[lane] = Some(p.clone());
                lane
            } else if let Some(existing) = lanes.iter().position(|l| l.as_deref() == Some(p.as_str())) {
                existing
            } else {
                match lanes.iter().position(Option::is_none) {
                    Some(free) => {
                        lanes[free] = Some(p.clone());
                        free
                    }
                    None => {
                        lanes.push(Some(p.clone()));
                        lanes.len() - 1
                    }
                }
            };
            edges.push(Edge { to_row, lane: travel });
        }
        while lanes.last().is_some_and(Option::is_none) {
            lanes.pop();
        }
        rows.push(Row {
            commit: c,
            lane,
            edges,
            through,
        });
    }
    rows
}

/// Read the whole history of all branches and lay it out.
pub fn history(git: &Git, limit: usize) -> Result<Vec<Row>> {
    let n = format!("-n{limit}");
    let text = git.run(&[
        "log",
        "--all",
        "--topo-order",
        &n,
        "--format=%H%x00%P%x00%D%x00%s%x00%an%x00%at%x00",
    ])?;
    Ok(layout(parse_log(&text)))
}

/// Line colours: an original palette chosen for contrast on white and in print.
const LINE_COLOURS: [&str; 8] = [
    "#d9480f", "#1971c2", "#2b8a3e", "#ae3ec9", "#e8590c", "#0c8599", "#c2255c", "#5c940d",
];

fn esc(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

/// Upper bound on the rendered width of `text` in a proportional sans-serif font at `size` px.
/// Uses 1.0 em per character (about the widest advance in Helvetica/Arial, e.g. "W" or "M") so
/// no subject is clipped, at the cost of some spare space for narrow text.
fn text_width(text: &str, size: f64) -> f64 {
    text.chars().count() as f64 * size
}

/// Width of a bold 12 px branch/tag badge: 5 px padding each side plus the same 1.0 em per
/// character upper bound as [`text_width`], so even wide-glyph names stay inside the pill.
fn badge_width(name: &str) -> f64 {
    10.0 + text_width(name, 12.0)
}

/// Draw the map as a standalone SVG.
pub fn subway_svg(rows: &[Row]) -> String {
    const ROW: f64 = 34.0;
    const LANE: f64 = 22.0;
    const TOP: f64 = 24.0;
    const LEFT: f64 = 20.0;
    let lanes = rows
        .iter()
        .flat_map(|r| r.edges.iter().map(|e| e.lane).chain([r.lane]))
        .max()
        .unwrap_or(0)
        + 1;
    let text_x = LEFT + lanes as f64 * LANE + 12.0;
    // Size the canvas from the widest row (badges + subject, or the id/author line) so long
    // branch names and subjects are never clipped; keep the old 560 px as a minimum.
    let content = rows
        .iter()
        .map(|r| {
            let badges: f64 = r.commit.refs.iter().map(|n| badge_width(n) + 6.0).sum();
            let subject = text_width(&r.commit.subject, 12.0);
            let meta = text_width(&format!("0000000 · {}", r.commit.author), 10.0);
            badges + subject.max(meta)
        })
        .fold(0.0, f64::max);
    let width = (text_x + content.max(548.0) + 12.0).ceil();
    let height = TOP * 2.0 + rows.len() as f64 * ROW;
    let x = |lane: usize| LEFT + lane as f64 * LANE;
    let y = |row: usize| TOP + row as f64 * ROW;
    let colour = |lane: usize| LINE_COLOURS[lane % LINE_COLOURS.len()];

    let mut s = String::new();
    let _ = write!(
        s,
        r##"<svg xmlns="http://www.w3.org/2000/svg" width="{width}" height="{height}" viewBox="0 0 {width} {height}" font-family="Helvetica, Arial, sans-serif" font-size="12"><rect width="100%" height="100%" fill="#ffffff"/>"##
    );
    // Lines first, stations on top.
    for (i, r) in rows.iter().enumerate() {
        for e in &r.edges {
            let target_lane = rows.get(e.to_row).map_or(e.lane, |p| p.lane);
            let (x0, y0, xt, x1, y1) = (x(r.lane), y(i), x(e.lane), x(target_lane), y(e.to_row));
            let bend = ROW * 0.5;
            let _ = write!(
                s,
                r#"<path d="M{x0} {y0} L{xt} {} L{xt} {} L{x1} {y1}" fill="none" stroke="{}" stroke-width="5" stroke-linejoin="round" stroke-linecap="round"/>"#,
                (y0 + bend).min(y1),
                (y1 - bend).max(y0 + bend).min(y1),
                colour(e.lane)
            );
        }
    }
    for (i, r) in rows.iter().enumerate() {
        let (cx, cy) = (x(r.lane), y(i));
        let merge = r.commit.parents.len() > 1;
        let radius = if merge { 7.0 } else { 6.0 };
        let _ = write!(
            s,
            r##"<circle cx="{cx}" cy="{cy}" r="{radius}" fill="#ffffff" stroke="{}" stroke-width="{}"/>"##,
            colour(r.lane),
            if r.commit.head { 5 } else { 3 }
        );
        let mut tx = text_x;
        for name in &r.commit.refs {
            let w = badge_width(name);
            let _ = write!(
                s,
                r##"<rect x="{tx}" y="{}" width="{w}" height="18" rx="9" fill="{}"/><text x="{}" y="{}" fill="#ffffff" font-weight="bold">{}</text>"##,
                cy - 9.0,
                colour(r.lane),
                tx + 5.0,
                cy + 4.0,
                esc(name)
            );
            tx += w + 6.0;
        }
        let short: String = r.commit.id.chars().take(7).collect();
        let _ = write!(
            s,
            r##"<text x="{tx}" y="{}" fill="#212529">{}</text><text x="{tx}" y="{}" fill="#868e96" font-size="10">{short} · {}</text>"##,
            cy - 1.0,
            esc(&r.commit.subject),
            cy + 11.0,
            esc(&r.commit.author)
        );
    }
    s.push_str("</svg>\n");
    s
}

/// Draw the map as terminal text, one row per commit.
pub fn subway_text(rows: &[Row]) -> String {
    let lanes = rows
        .iter()
        .flat_map(|r| r.edges.iter().map(|e| e.lane).chain([r.lane]))
        .max()
        .unwrap_or(0)
        + 1;
    let mut out = String::new();
    for r in rows {
        let mut cells = vec![' '; lanes];
        for l in &r.through {
            if *l < lanes {
                cells[*l] = '│';
            }
        }
        cells[r.lane] = if r.commit.head {
            '◉'
        } else if r.commit.parents.len() > 1 {
            '◆'
        } else {
            '●'
        };
        let lanes_s: String = cells.iter().flat_map(|c| [*c, ' ']).collect();
        let refs = if r.commit.refs.is_empty() {
            String::new()
        } else {
            format!("[{}] ", r.commit.refs.join(", "))
        };
        let short: String = r.commit.id.chars().take(7).collect();
        let _ = writeln!(out, "{lanes_s} {short} {refs}{}", r.commit.subject);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn c(id: &str, parents: &[&str]) -> Commit {
        Commit {
            id: id.into(),
            parents: parents.iter().map(|p| p.to_string()).collect(),
            refs: vec![],
            head: false,
            subject: id.into(),
            author: "a".into(),
            time: 0,
        }
    }

    #[test]
    fn branch_and_merge_use_two_lanes() {
        // m merges b2 into a2; history: a1 <- a2, a1 <- b1 <- b2.
        let rows = layout(vec![
            c("m", &["a2", "b2"]),
            c("b2", &["b1"]),
            c("a2", &["a1"]),
            c("b1", &["a1"]),
            c("a1", &[]),
        ]);
        assert_eq!(rows[0].lane, 0);
        assert_eq!(rows[1].lane, 1, "the merged branch runs beside main");
        assert_eq!(rows[2].lane, 0);
        assert_eq!(rows[4].lane, 0, "both lines end at the root");
        assert!(rows[2].through.contains(&1));
        let svg = subway_svg(&rows);
        assert!(svg.starts_with("<svg") && svg.contains("<circle"));
        assert_eq!(subway_text(&rows).lines().count(), 5);
    }

    fn svg_width(svg: &str) -> f64 {
        let rest = &svg[svg.find("width=\"").map_or(0, |i| i + 7)..];
        rest[..rest.find('"').unwrap_or(0)].parse().unwrap_or(0.0)
    }

    #[test]
    fn svg_grows_to_fit_long_subjects_and_badges() {
        let short = subway_svg(&layout(vec![c("a", &[])]));
        assert!((svg_width(&short) - 614.0).abs() < 1.0, "short logs keep the old width");

        let mut long = c("b", &[]);
        long.subject = "x".repeat(200);
        long.refs = vec!["feature/a-very-long-branch-name".into(), "#v1.2.3".into()];
        let rows = layout(vec![long]);
        let svg = subway_svg(&rows);
        let text_x = 20.0 + 22.0 + 12.0;
        let badges = badge_width(&rows[0].commit.refs[0]) + badge_width(&rows[0].commit.refs[1]) + 12.0;
        let need = text_x + badges + 200.0 * 12.0;
        assert!(svg_width(&svg) >= need, "width {} < needed {need}", svg_width(&svg));
    }

    #[test]
    fn svg_fits_wide_glyph_subjects() {
        let mut wide = c("w", &[]);
        wide.subject = "W".repeat(200);
        let svg = subway_svg(&layout(vec![wide]));
        let text_x = 20.0 + 22.0 + 12.0;
        let need = text_x + 200.0 * 1.0 * 12.0;
        assert!(svg_width(&svg) >= need, "width {} < needed {need}", svg_width(&svg));
    }

    #[test]
    fn svg_fits_wide_glyph_badges() {
        let mut wide = c("w", &[]);
        wide.refs = vec!["W".repeat(200)];
        let svg = subway_svg(&layout(vec![wide]));
        let text_x = 20.0 + 22.0 + 12.0;
        let badge = 10.0 + 200.0 * 1.0 * 12.0;
        assert!(
            svg.contains(&format!(r#"width="{badge}""#)),
            "badge rect grows with the name"
        );
        let need = text_x + badge;
        assert!(svg_width(&svg) >= need, "width {} < needed {need}", svg_width(&svg));
    }

    #[test]
    fn parses_git_log_records() {
        let text = "h1\x00p1 p2\x00HEAD -> main, tag: v1\x00Merge x\x00Ann\x00100\x00\nh2\x00\x00\x00Root\x00Ann\x0050\x00\n";
        let cs = parse_log(text);
        assert_eq!(cs.len(), 2);
        assert!(cs[0].head);
        assert_eq!(cs[0].refs, vec!["main", "#v1"]);
        assert_eq!(cs[0].parents.len(), 2);
        assert!(cs[1].parents.is_empty());
    }
}
