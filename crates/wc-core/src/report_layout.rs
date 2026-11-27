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

    /// A layout given as JSON (as the assistant and `wiringcraft report-layout set` take it),
    /// for the report `kind`: the keys are those of a `[[report]]` table, and an unknown key
    /// (of the layout or of a column) is refused rather than dropped. A `kind` in the JSON is
    /// replaced by `kind`.
    pub fn from_json(kind: &str, value: serde_json::Value) -> Result<ReportLayout, String> {
        use serde_json::{Value, json};
        let mut value = value;
        let Some(fields) = value.as_object_mut() else {
            return Err("The layout must be an object.".into());
        };
        let keys = |v: Value| -> Vec<String> { v.as_object().map(|o| o.keys().cloned().collect()).unwrap_or_default() };
        let column = ReportColumn {
            from: "x".into(),
            heading: Some("x".into()),
        };
        let known = keys(json!(ReportLayout {
            kind: "x".into(),
            title: Some("x".into()),
            columns: vec![column.clone()],
            sort: vec!["x".into()],
            group: Some("x".into()),
            header: vec!["x".into()],
        }));
        let known_column = keys(json!(column));
        let unknown = |fields: &serde_json::Map<String, Value>, known: &[String], what: &str| match fields
            .keys()
            .find(|k| !known.contains(k))
        {
            Some(k) => Err(format!(
                "Unknown {what} key {k:?}; known keys are {}.",
                known.join(", ")
            )),
            None => Ok(()),
        };
        unknown(fields, &known, "layout")?;
        if let Some(Value::Array(columns)) = fields.get("column") {
            for c in columns.iter().filter_map(Value::as_object) {
                unknown(c, &known_column, "column")?;
            }
        }
        fields.insert("kind".into(), json!(kind));
        serde_json::from_value(value).map_err(|e| format!("Bad layout: {e}"))
    }

    /// The layout as it is kept in `project.toml` for a report whose built-in columns are
    /// `builtin`, as the report layout dialog keeps it: texts trimmed, empty ones dropped, no
    /// columns when they are the built-in ones unchanged, and `None` when nothing is left but
    /// the built-in layout. Refused when the report would show no column.
    pub fn kept(&self, builtin: &[String]) -> Result<Option<ReportLayout>, String> {
        let set = |v: &str| Some(v.trim().to_owned()).filter(|v| !v.is_empty());
        if self.columns.is_empty() && builtin.is_empty() {
            return Err("Show at least one column.".into());
        }
        let as_builtin = self.columns.is_empty()
            || (self.columns.len() == builtin.len()
                && self
                    .columns
                    .iter()
                    .zip(builtin)
                    .all(|(c, b)| &c.from == b && c.heading.as_deref().is_none_or(|h| h.trim().is_empty())));
        let columns = if as_builtin {
            Vec::new()
        } else {
            self.columns
                .iter()
                .map(|c| ReportColumn {
                    from: c.from.clone(),
                    heading: c.heading.as_deref().and_then(set),
                })
                .collect()
        };
        let layout = ReportLayout {
            kind: self.kind.clone(),
            title: self.title.as_deref().and_then(set),
            columns,
            sort: self
                .sort
                .iter()
                .map(|entry| ReportLayout::sort_key(entry.trim()))
                .filter(|(column, _)| !column.is_empty())
                .map(|(column, descending)| format!("{}{column}", if descending { "-" } else { "" }))
                .collect(),
            group: self.group.as_deref().and_then(set),
            header: self.header.iter().filter_map(|k| set(k)).collect(),
        };
        let builtin_only = ReportLayout {
            kind: layout.kind.clone(),
            ..ReportLayout::default()
        };
        Ok((layout != builtin_only).then_some(layout))
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

    #[test]
    fn a_layout_from_json_refuses_unknown_keys_and_takes_the_kind_given() {
        use super::ReportLayout;
        use serde_json::json;
        let l = ReportLayout::from_json(
            "parts",
            json!({"kind": "wires", "title": "BOM", "column": [{"from": "Qty"}]}),
        )
        .unwrap();
        assert_eq!(l.kind, "parts");
        assert_eq!(l.title.as_deref(), Some("BOM"));
        let e = ReportLayout::from_json("parts", json!({"titel": "x"})).unwrap_err();
        assert!(e.contains("Unknown layout key \"titel\""), "{e}");
        let e = ReportLayout::from_json("parts", json!({"column": [{"form": "Qty"}]})).unwrap_err();
        assert!(e.contains("Unknown column key \"form\""), "{e}");
        assert!(ReportLayout::from_json("parts", json!([1])).is_err());
        assert!(
            ReportLayout::from_json("parts", json!({"sort": "Qty"}))
                .unwrap_err()
                .starts_with("Bad layout")
        );
    }

    #[test]
    fn a_kept_layout_is_trimmed_and_the_built_in_one_keeps_none() {
        use super::{ReportColumn, ReportLayout};
        let builtin = ["Tag".to_owned(), "Qty".to_owned()];
        let col = |from: &str, heading: Option<&str>| ReportColumn {
            from: from.into(),
            heading: heading.map(str::to_owned),
        };
        // Only blanks, or the built-in columns spelled out: the built-in layout.
        let blank = ReportLayout {
            kind: "parts".into(),
            title: Some("  ".into()),
            columns: vec![col("Tag", Some(" ")), col("Qty", None)],
            sort: vec![" ".into(), "-".into()],
            group: Some(String::new()),
            header: vec![" ".into()],
        };
        assert_eq!(blank.kept(&builtin).unwrap(), None);
        let given = ReportLayout {
            kind: "parts".into(),
            title: Some(" BOM ".into()),
            columns: vec![col("Qty", Some(" Count ")), col("Tag", Some(""))],
            sort: vec![" - Qty ".into(), "Tag".into()],
            group: Some(" Tag".into()),
            header: vec![" customer, site ".into(), "".into()],
        };
        let kept = given.kept(&builtin).unwrap().unwrap();
        assert_eq!(kept.title.as_deref(), Some("BOM"));
        assert_eq!(kept.columns, [col("Qty", Some("Count")), col("Tag", None)]);
        assert_eq!(kept.sort, ["-Qty", "Tag"]);
        assert_eq!(kept.group.as_deref(), Some("Tag"));
        // A key with a comma stays one key.
        assert_eq!(kept.header, ["customer, site"]);
        // No column to show.
        let none = ReportLayout {
            kind: "parts".into(),
            ..ReportLayout::default()
        };
        assert!(none.kept(&[]).is_err());
    }
}

#[cfg(test)]
mod edge_tests {
    use super::{ReportColumn, ReportLayout};
    use serde_json::json;

    fn col(from: &str, heading: Option<&str>) -> ReportColumn {
        ReportColumn {
            from: from.into(),
            heading: heading.map(str::to_owned),
        }
    }

    fn builtin() -> Vec<String> {
        vec!["Tag".to_owned(), "Qty".to_owned()]
    }

    #[test]
    fn sort_keys_trim_and_take_only_one_minus() {
        assert_eq!(ReportLayout::sort_key(" Qty "), ("Qty", false));
        assert_eq!(ReportLayout::sort_key("- Qty"), ("Qty", true));
        assert_eq!(ReportLayout::sort_key("--Qty"), ("-Qty", true));
        assert_eq!(ReportLayout::sort_key("-"), ("", true));
        assert_eq!(ReportLayout::sort_key(""), ("", false));
    }

    #[test]
    fn built_in_columns_reordered_cut_or_renamed_are_kept() {
        let layout = |columns| ReportLayout {
            kind: "parts".into(),
            columns,
            ..ReportLayout::default()
        };
        for columns in [
            vec![col("Qty", None), col("Tag", None)],
            vec![col("Tag", None)],
            vec![col("Tag", Some("Ref")), col("Qty", None)],
            vec![col("Tag", None), col("Qty", None), col("Tag", None)],
        ] {
            let Ok(Some(kept)) = layout(columns.clone()).kept(&builtin()) else {
                unreachable!("{columns:?}")
            };
            assert_eq!(kept.columns, columns);
            assert_eq!(kept.kind, "parts");
        }
    }

    #[test]
    fn built_in_columns_with_a_sort_keep_the_sort_and_no_columns() {
        let l = ReportLayout {
            kind: "wires".into(),
            columns: vec![col("Tag", None), col("Qty", Some("  "))],
            sort: vec!["-Qty".into()],
            ..ReportLayout::default()
        };
        let Ok(Some(kept)) = l.kept(&builtin()) else {
            unreachable!()
        };
        assert!(kept.columns.is_empty());
        assert_eq!(kept.sort, ["-Qty"]);
        assert_eq!(kept.kind, "wires");
        // Kept twice gives the same layout.
        assert_eq!(kept.kept(&builtin()), Ok(Some(kept.clone())));
    }

    #[test]
    fn columns_given_for_a_report_without_built_in_ones_are_kept() {
        let l = ReportLayout {
            kind: "plc".into(),
            columns: vec![col("Address", None)],
            ..ReportLayout::default()
        };
        let Ok(Some(kept)) = l.kept(&[]) else { unreachable!() };
        assert_eq!(kept.columns, [col("Address", None)]);
    }

    #[test]
    fn json_checks_each_column_and_refuses_a_bad_column_list() {
        let e = ReportLayout::from_json(
            "parts",
            json!({"column": [{"from": "Tag"}, {"from": "Qty", "head": "x"}]}),
        );
        let Err(e) = e else { unreachable!() };
        assert!(e.contains("Unknown column key \"head\""), "{e}");
        assert!(e.contains("from") && e.contains("heading"), "names the known keys: {e}");
        let Err(e) = ReportLayout::from_json("parts", json!({"column": {"from": "Qty"}})) else {
            unreachable!()
        };
        assert!(e.starts_with("Bad layout"), "{e}");
        let Err(e) = ReportLayout::from_json("parts", json!({"column": ["Qty"]})) else {
            unreachable!()
        };
        assert!(e.starts_with("Bad layout"), "{e}");
        let Err(e) = ReportLayout::from_json("parts", json!("parts")) else {
            unreachable!()
        };
        assert_eq!(e, "The layout must be an object.");
    }

    #[test]
    fn an_empty_json_object_is_the_built_in_layout_of_the_kind() {
        let Ok(l) = ReportLayout::from_json("cables", json!({})) else {
            unreachable!()
        };
        assert_eq!(
            l,
            ReportLayout {
                kind: "cables".into(),
                ..ReportLayout::default()
            }
        );
        assert_eq!(l.kept(&builtin()), Ok(None));
    }

    #[test]
    fn a_full_json_layout_reads_every_key() {
        let Ok(l) = ReportLayout::from_json(
            "parts",
            json!({
                "title": "BOM",
                "column": [{"from": "Qty", "heading": "Count"}],
                "sort": ["-Qty", "Tag"],
                "group": "Tag",
                "header": ["project.name"],
            }),
        ) else {
            unreachable!()
        };
        assert_eq!(l.columns, [col("Qty", Some("Count"))]);
        assert_eq!(l.sort, ["-Qty", "Tag"]);
        assert_eq!(l.group.as_deref(), Some("Tag"));
        assert_eq!(l.header, ["project.name"]);
        let Ok(back) = ReportLayout::from_json("parts", json!(l)) else {
            unreachable!()
        };
        assert_eq!(back, l);
    }
}
