//! Report layouts the user designs: which columns a report shows, in what order, under which
//! headings, how its rows are sorted and grouped, and which title-block values its pages print
//! above the table.
//!
//! They are kept in `project.toml` as `[[report]]` tables, one per report kind (`parts`,
//! `wires`, `terminals`, `cables`, `plc`, ...). A report without one keeps its built-in layout.
//!
//! ```toml
//! [[report]]
//! kind = "parts"
//! title = "Bill of materials"
//! sort = ["Manufacturer", "-Qty"]
//! group = "Manufacturer"
//! header = ["project.name", "revision"]
//!
//! [[report.column]]
//! from = "Order number"
//! heading = "Art. no."
//!
//! [[report.column]]
//! from = "Qty"
//! ```

use serde::{Deserialize, Serialize};

/// The layout of one report kind.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct ReportLayout {
    /// The report kind it applies to, by its name (`parts`, `wires`, ...).
    pub kind: String,
    /// A title in place of the built-in one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    /// The columns to show, in this order. Empty keeps the built-in columns.
    #[serde(default, rename = "column", skip_serializing_if = "Vec::is_empty")]
    pub columns: Vec<ReportColumn>,
    /// Columns to sort the rows by, most significant first, by their built-in heading. A `-`
    /// in front sorts that column descending. Numbers inside texts sort by value (`X2` before
    /// `X10`). Rows equal in every sort column keep the built-in order.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub sort: Vec<String>,
    /// A column to group the rows by, by its built-in heading. Rows of one value come together
    /// (in the order of their first row after sorting); the PDF prints a heading with the
    /// value above each group and leaves the column out of the rows, while a CSV keeps one row
    /// per record with the value in every row.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub group: Option<String>,
    /// Values printed above the table on each of the report's pages, as `Label: value`, by
    /// the keys a title block uses: `project.name`, or a title-block field such as `revision`,
    /// `location` or `customer`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub header: Vec<String>,
}

/// One column of a [`ReportLayout`].
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct ReportColumn {
    /// The built-in column it shows, by its built-in heading.
    pub from: String,
    /// A heading in place of the built-in one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub heading: Option<String>,
}

impl ReportLayout {
    /// A sort entry split into its column and whether it sorts descending.
    pub fn sort_key(entry: &str) -> (&str, bool) {
        match entry.strip_prefix('-') {
            Some(column) => (column.trim(), true),
            None => (entry.trim(), false),
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use crate::project::ProjectMeta;

    #[test]
    fn layouts_read_and_write_as_report_tables_in_project_toml() {
        let text = r#"
name = "x"

[[report]]
kind = "parts"
title = "Bill of materials"
sort = ["Manufacturer", "-Qty"]
group = "Manufacturer"
header = ["project.name", "revision"]

[[report.column]]
from = "Order number"
heading = "Art. no."

[[report.column]]
from = "Qty"
"#;
        let meta: ProjectMeta = toml::from_str(text).unwrap();
        let l = &meta.reports[0];
        assert_eq!(l.kind, "parts");
        assert_eq!(l.columns[0].heading.as_deref(), Some("Art. no."));
        assert_eq!(l.columns[1].heading, None);
        assert_eq!(super::ReportLayout::sort_key(&l.sort[1]), ("Qty", true));
        assert_eq!(l.group.as_deref(), Some("Manufacturer"));
        assert_eq!(l.header, ["project.name", "revision"]);
        let again: ProjectMeta = toml::from_str(&toml::to_string(&meta).unwrap()).unwrap();
        assert_eq!(again.reports, meta.reports);
        // A project without layouts writes none.
        assert!(!toml::to_string(&ProjectMeta::new("y")).unwrap().contains("report"));
    }
}
