//! The report layout dialog: one report's `[[report]]` table ([`wc_core::report_layout`]) as a
//! form: its title, which columns show in what order under which headings, the sort columns
//! and their direction, the column to group by and the title-block values printed above the
//! table. The Reports dialog shows the report in the draft layout while it is edited.

use eframe::egui;
use wc_core::report_layout::{ReportColumn, ReportLayout};

/// One column of the report as the dialog edits it.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct ColumnRow {
    /// The built-in heading it shows.
    pub from: String,
    /// The heading to print in place of the built-in one; empty keeps it.
    pub heading: String,
    /// Whether the report shows it.
    pub shown: bool,
}

/// One sort column: its built-in heading and whether it sorts descending.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct SortRow {
    pub column: String,
    pub descending: bool,
}

/// The layout of one report kind, being edited.
#[derive(Debug, Clone, PartialEq)]
pub struct Form {
    pub kind: wc_report::Kind,
    /// The report's built-in columns, in their built-in order.
    pub builtin: Vec<String>,
    /// A title in place of the built-in one; empty keeps it.
    pub title: String,
    /// Every column: the shown ones first, in the order they print, then the others.
    pub columns: Vec<ColumnRow>,
    pub sort: Vec<SortRow>,
    /// The column rows are grouped by; empty for none.
    pub group: String,
    /// Title-block keys printed above the table, comma separated (`project.name, revision`).
    pub header: String,
    /// The layout the project had for this report when the dialog opened.
    pub loaded: Option<ReportLayout>,
}

impl Form {
    /// The form for `kind`, on the project's layout for it (`loaded`) and the report's built-in
    /// columns. A column the layout names that the report does not have stays in the list, so
    /// OK does not drop it unseen.
    pub fn new(kind: wc_report::Kind, builtin: Vec<String>, loaded: Option<&ReportLayout>) -> Form {
        let mut columns: Vec<ColumnRow> = Vec::new();
        let picked = loaded.map(|l| l.columns.as_slice()).unwrap_or_default();
        if picked.is_empty() {
            columns.extend(builtin.iter().map(|c| ColumnRow {
                from: c.clone(),
                heading: String::new(),
                shown: true,
            }));
        } else {
            for c in picked {
                columns.push(ColumnRow {
                    from: c.from.clone(),
                    heading: c.heading.clone().unwrap_or_default(),
                    shown: true,
                });
            }
            for c in &builtin {
                if !columns.iter().any(|r| &r.from == c) {
                    columns.push(ColumnRow {
                        from: c.clone(),
                        heading: String::new(),
                        shown: false,
                    });
                }
            }
        }
        let sort = loaded
            .map(|l| {
                l.sort
                    .iter()
                    .map(|entry| {
                        let (column, descending) = ReportLayout::sort_key(entry);
                        SortRow {
                            column: column.to_owned(),
                            descending,
                        }
                    })
                    .collect()
            })
            .unwrap_or_default();
        Form {
            kind,
            builtin,
            title: loaded.and_then(|l| l.title.clone()).unwrap_or_default(),
            columns,
            sort,
            group: loaded.and_then(|l| l.group.clone()).unwrap_or_default(),
            header: loaded.map(|l| l.header.join(", ")).unwrap_or_default(),
            loaded: loaded.cloned(),
        }
    }

    /// The layout the form describes, or `None` when it is the built-in one (no layout is kept
    /// then). Columns are written only when they differ from the built-in ones.
    pub fn layout(&self) -> Option<ReportLayout> {
        let set = |v: &str| Some(v.trim().to_owned()).filter(|v| !v.is_empty());
        let shown: Vec<&ColumnRow> = self.columns.iter().filter(|c| c.shown).collect();
        let as_builtin = shown.len() == self.builtin.len()
            && shown
                .iter()
                .zip(&self.builtin)
                .all(|(c, b)| &c.from == b && c.heading.trim().is_empty());
        let columns = if as_builtin {
            Vec::new()
        } else {
            shown
                .iter()
                .map(|c| ReportColumn {
                    from: c.from.clone(),
                    heading: set(&c.heading),
                })
                .collect()
        };
        let layout = ReportLayout {
            kind: self.kind.name().to_owned(),
            title: set(&self.title),
            columns,
            sort: self
                .sort
                .iter()
                .filter(|s| !s.column.trim().is_empty())
                .map(|s| format!("{}{}", if s.descending { "-" } else { "" }, s.column.trim()))
                .collect(),
            group: set(&self.group),
            header: self
                .header
                .split(',')
                .map(str::trim)
                .filter(|k| !k.is_empty())
                .map(str::to_owned)
                .collect(),
        };
        (layout
            != ReportLayout {
                kind: layout.kind.clone(),
                ..ReportLayout::default()
            })
        .then_some(layout)
    }

    /// Why the layout cannot be kept: no column is shown.
    pub fn problem(&self) -> Option<String> {
        (!self.columns.iter().any(|c| c.shown)).then(|| "Show at least one column.".to_owned())
    }

    /// Move column `i` one place up (`-1`) or down (`1`).
    pub fn move_column(&mut self, i: usize, by: isize) {
        let Some(j) = i.checked_add_signed(by).filter(|j| *j < self.columns.len()) else {
            return;
        };
        self.columns.swap(i, j);
    }
}

/// The form's fields. A column of the layout that the report does not have is marked.
pub fn show(ui: &mut egui::Ui, form: &mut Form) {
    let theme = crate::theme::Theme::of(ui.ctx());
    egui::Grid::new("reportlayout-top")
        .num_columns(2)
        .spacing([8.0, 6.0])
        .show(ui, |ui| {
            ui.label("Title");
            ui.add(
                egui::TextEdit::singleline(&mut form.title)
                    .hint_text("as built in")
                    .desired_width(260.0),
            );
            ui.end_row();
        });
    ui.add_space(4.0);
    ui.label(egui::RichText::new("Columns").strong());
    let mut moved = None;
    egui::ScrollArea::vertical()
        .id_salt("reportlayout-columns")
        .max_height(200.0)
        .auto_shrink([false, true])
        .show(ui, |ui| {
            egui::Grid::new("reportlayout-columns-grid")
                .num_columns(4)
                .spacing([8.0, 4.0])
                .show(ui, |ui| {
                    let n = form.columns.len();
                    for (i, c) in form.columns.iter_mut().enumerate() {
                        let known = form.builtin.contains(&c.from);
                        let mut label = egui::RichText::new(&c.from);
                        if !known {
                            label = label.color(theme.error);
                        }
                        ui.checkbox(&mut c.shown, label).on_hover_text(if known {
                            "Show this column"
                        } else {
                            "This report has no such column; it is left out"
                        });
                        ui.add(
                            egui::TextEdit::singleline(&mut c.heading)
                                .hint_text("heading as built in")
                                .desired_width(160.0),
                        );
                        if ui.add_enabled(i > 0, egui::Button::new("Up")).clicked() {
                            moved = Some((i, -1));
                        }
                        if ui.add_enabled(i + 1 < n, egui::Button::new("Down")).clicked() {
                            moved = Some((i, 1));
                        }
                        ui.end_row();
                    }
                });
        });
    if let Some((i, by)) = moved {
        form.move_column(i, by);
    }
    ui.add_space(4.0);
    ui.label(egui::RichText::new("Sort by").strong());
    let mut drop = None;
    for (i, s) in form.sort.iter_mut().enumerate() {
        ui.horizontal(|ui| {
            pick_column(ui, ("reportlayout-sort", i), &mut s.column, &form.builtin, false);
            ui.checkbox(&mut s.descending, "Descending");
            if ui.button("Remove").clicked() {
                drop = Some(i);
            }
        });
    }
    if let Some(i) = drop {
        form.sort.remove(i);
    }
    if ui.button("Add sort column").clicked()
        && let Some(first) = form.builtin.first()
    {
        form.sort.push(SortRow {
            column: first.clone(),
            descending: false,
        });
    }
    ui.add_space(4.0);
    ui.horizontal(|ui| {
        ui.label("Group by");
        pick_column(ui, "reportlayout-group", &mut form.group, &form.builtin, true);
    });
    ui.horizontal(|ui| {
        ui.label("Above the table");
        ui.add(
            egui::TextEdit::singleline(&mut form.header)
                .hint_text("project.name, revision")
                .desired_width(260.0),
        )
        .on_hover_text("Title-block values printed above the table on each page, by their keys, comma separated");
    });
}

/// A combo box of the report's columns; `none` adds a "(none)" entry for an empty value.
fn pick_column(ui: &mut egui::Ui, id: impl std::hash::Hash, value: &mut String, columns: &[String], none: bool) {
    let shown = if value.is_empty() { "(none)" } else { value.as_str() }.to_owned();
    egui::ComboBox::from_id_salt(id).selected_text(shown).show_ui(ui, |ui| {
        if none {
            ui.selectable_value(value, String::new(), "(none)");
        }
        for c in columns {
            ui.selectable_value(value, c.clone(), c);
        }
    });
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    fn builtin() -> Vec<String> {
        ["Tag", "Part", "Qty"].iter().map(|s| (*s).to_owned()).collect()
    }

    #[test]
    fn an_untouched_form_keeps_no_layout() {
        let f = Form::new(wc_report::Kind::Parts, builtin(), None);
        assert_eq!(f.columns.iter().filter(|c| c.shown).count(), 3);
        assert_eq!(f.layout(), None);
        assert_eq!(f.problem(), None);
    }

    #[test]
    fn columns_sort_group_and_header_make_a_layout_that_reads_back() {
        let mut f = Form::new(wc_report::Kind::Parts, builtin(), None);
        f.title = " Bill of materials ".into();
        f.columns[1].shown = false;
        f.move_column(2, -1);
        f.move_column(1, -1);
        f.columns[0].heading = "Amount".into();
        f.sort.push(SortRow {
            column: "Qty".into(),
            descending: true,
        });
        f.group = "Tag".into();
        f.header = "project.name, , revision".into();
        let l = f.layout().unwrap();
        assert_eq!(l.kind, "parts");
        assert_eq!(l.title.as_deref(), Some("Bill of materials"));
        let cols: Vec<(&str, Option<&str>)> = l
            .columns
            .iter()
            .map(|c| (c.from.as_str(), c.heading.as_deref()))
            .collect();
        assert_eq!(cols, [("Qty", Some("Amount")), ("Tag", None)]);
        assert_eq!(l.sort, ["-Qty"]);
        assert_eq!(l.group.as_deref(), Some("Tag"));
        assert_eq!(l.header, ["project.name", "revision"]);
        // Opened again on it, the form shows the same layout.
        let again = Form::new(wc_report::Kind::Parts, builtin(), Some(&l));
        assert_eq!(again.layout(), Some(l));
        assert!(!again.columns[2].shown, "the hidden column is listed, unticked");
    }

    #[test]
    fn a_column_the_report_lacks_stays_listed_and_no_column_is_refused() {
        let l = ReportLayout {
            kind: "parts".into(),
            columns: vec![ReportColumn {
                from: "Colour".into(),
                heading: None,
            }],
            ..ReportLayout::default()
        };
        let mut f = Form::new(wc_report::Kind::Parts, builtin(), Some(&l));
        assert_eq!(f.columns[0].from, "Colour");
        assert_eq!(f.layout(), Some(l));
        for c in &mut f.columns {
            c.shown = false;
        }
        assert!(f.problem().is_some());
        f.move_column(0, -1);
        f.move_column(3, 1);
        assert_eq!(f.columns[0].from, "Colour", "moves past either end do nothing");
    }
}
