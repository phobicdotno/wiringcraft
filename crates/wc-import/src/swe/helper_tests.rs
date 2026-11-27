//! Tests for the small pure helpers of the SolidWorks Electrical import: dates, column lookup,
//! terminal levels, page sizes, segment hits, slugs, unique tags and page numbers.
#![allow(clippy::unwrap_used)]

use super::*;

#[test]
fn source_date_formats_digits_and_keeps_anything_else() {
    assert_eq!(source_date("20240131 10:00:00"), "2024-01-31");
    assert_eq!(source_date("  20240131  "), "2024-01-31");
    assert_eq!(source_date("2024-01-31"), "2024-01-31");
    assert_eq!(source_date("31.01.2024"), "31.01.2024");
    // Too short for a day: kept as written.
    assert_eq!(source_date("2024"), "2024");
    assert_eq!(source_date("   "), "");
}

#[test]
fn column_named_follows_name_order_and_strips_prefix() {
    let cols: Vec<String> = ["rev_id", "rev_createdby", "rev_orderno", "rev_fil_id"]
        .iter()
        .map(|s| (*s).to_owned())
        .collect();
    assert_eq!(column_named(&cols, &["orderno", "id"]), Some("rev_orderno"));
    assert_eq!(column_named(&cols, &["missing", "id"]), Some("rev_id"));
    // Only the first `_` is the table prefix.
    assert_eq!(column_named(&cols, &["fil_id"]), Some("rev_fil_id"));
    // The full name matches too.
    assert_eq!(column_named(&cols, &["rev_createdby"]), Some("rev_createdby"));
    assert_eq!(column_named(&cols, &["date"]), None);
    assert_eq!(column_named(&[], &["id"]), None);
}

#[test]
fn terminal_level_replaces_separators_and_rejects_blank() {
    assert_eq!(terminal_level("1.2").as_deref(), Some("1_2"));
    assert_eq!(terminal_level(" 1+2 ").as_deref(), Some("1_2"));
    assert_eq!(terminal_level("A1_x").as_deref(), Some("A1_x"));
    assert_eq!(terminal_level("PE").as_deref(), Some("PE"));
    assert_eq!(terminal_level(""), None);
    assert_eq!(terminal_level("   "), None);
}

#[test]
fn page_size_picks_smallest_sheet_that_fits() {
    let fits = |x, y| page_size(Point::new(x, y));
    assert!(matches!(fits(400.0, 290.0), (PageSize::A3, true)));
    // Within the snap tolerance of A3 still counts as A3.
    assert!(matches!(fits(420.04, 297.04), (PageSize::A3, true)));
    assert!(matches!(fits(421.0, 200.0), (PageSize::A2, true)));
    assert!(matches!(fits(594.0, 420.0), (PageSize::A2, true)));
    assert!(matches!(fits(600.0, 400.0), (PageSize::A2, false)));
}

#[test]
fn on_segment_only_hits_strictly_inside() {
    let a = Point::new(0.0, 0.0);
    let b = Point::new(10.0, 0.0);
    assert_eq!(on_segment(a, b, Point::new(5.0, 0.0)), Some(0.5));
    assert_eq!(on_segment(a, b, Point::new(2.5, 0.2)), Some(0.25));
    // Too far off the line.
    assert_eq!(on_segment(a, b, Point::new(5.0, 1.0)), None);
    // Near an end is a join at that end, not a hit along the line.
    assert_eq!(on_segment(a, b, Point::new(0.3, 0.0)), None);
    assert_eq!(on_segment(a, b, Point::new(9.8, 0.0)), None);
    // Beyond the ends.
    assert_eq!(on_segment(a, b, Point::new(15.0, 0.0)), None);
    assert_eq!(on_segment(a, b, Point::new(-3.0, 0.0)), None);
    // A degenerate segment has no inside.
    assert_eq!(on_segment(a, Point::new(0.01, 0.0), Point::new(0.005, 0.0)), None);
}

#[test]
fn slug_lowercases_collapses_dashes_and_falls_back() {
    assert_eq!(slug("Motor 3~ Starter"), "motor-3-starter");
    assert_eq!(slug("--A__B--"), "a-b");
    assert_eq!(slug("Ωµ"), "symbol");
    assert_eq!(slug(""), "symbol");
    let long = "x".repeat(100);
    assert_eq!(slug(&long).len(), 48);
}

#[test]
fn unique_suffixes_repeated_tags_and_counts_them() {
    let mut taken = BTreeSet::new();
    let mut renamed = 0;
    let k1 = DeviceTag::parse("=F1-K1").unwrap();
    assert_eq!(unique(k1.clone(), &mut taken, &mut renamed).to_string(), "=F1-K1");
    assert_eq!(unique(k1.clone(), &mut taken, &mut renamed).to_string(), "=F1-K1_2");
    assert_eq!(unique(k1, &mut taken, &mut renamed).to_string(), "=F1-K1_3");
    assert_eq!(renamed, 2);
    // A different function aspect is a different tag.
    let other = DeviceTag::parse("=F2-K1").unwrap();
    assert_eq!(unique(other, &mut taken, &mut renamed).to_string(), "=F2-K1");
    assert_eq!(renamed, 2);
}

fn page(label: &str, book: &str) -> SwePage {
    SwePage {
        label: label.to_owned(),
        book: book.to_owned(),
        ..SwePage::default()
    }
}

#[test]
fn page_numbers_use_labels_books_and_dedupe() {
    let pages = [page("01", "1"), page(" ", "1"), page("01", "2"), page("01", "1")];
    let refs: Vec<&SwePage> = pages.iter().collect();

    let mut renumbered = 0;
    assert_eq!(
        page_numbers(&refs, 1, None, &mut renumbered),
        ["01", "2", "01-2", "01-3"]
    );
    assert_eq!(renumbered, 2);

    let mut renumbered = 0;
    assert_eq!(
        page_numbers(&refs, 2, None, &mut renumbered),
        ["1.01", "1.2", "2.01", "1.01-2"]
    );
    assert_eq!(renumbered, 1);
}

#[test]
fn page_numbers_from_first_number_ignore_labels() {
    let pages = [page("07", "1"), page("07", "1"), page("", "2")];
    let refs: Vec<&SwePage> = pages.iter().collect();
    let mut renumbered = 0;
    assert_eq!(page_numbers(&refs, 2, Some(5), &mut renumbered), ["5", "6", "7"]);
    assert_eq!(renumbered, 0);
    // The count saturates rather than overflowing.
    let mut renumbered = 0;
    let numbers = page_numbers(&refs, 1, Some(usize::MAX), &mut renumbered);
    let max = usize::MAX.to_string();
    assert_eq!(numbers, [max.clone(), format!("{max}-2"), format!("{max}-3")]);
    assert_eq!(renumbered, 2);
}
