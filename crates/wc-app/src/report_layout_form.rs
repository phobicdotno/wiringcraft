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
    /// Title-block keys printed above the table, comma separated (`project.name, revision`);
    /// a key with a comma, a leading quote or spaces around it is in double quotes, a quote in
    /// it doubled ([`join_keys`], [`split_keys`]).
    pub header: String,
    /// The layout the project had for this report when the dialog opened.
    pub loaded: Option<ReportLayout>,
    /// [`Form::layout`] as the dialog opened: OK changes nothing while the form still gives
    /// it (a loaded layout listing every built-in column gives none, as it shows the same).
    pub initial: Option<ReportLayout>,
    /// "Built-in layout" was pressed: OK then drops the project's layout for the report even
    /// when the one it opened on showed the same as the built-in one.
    pub reset: bool,
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
        let mut form = Form {
            kind,
            builtin,
            title: loaded.and_then(|l| l.title.clone()).unwrap_or_default(),
            columns,
            sort,
            group: loaded.and_then(|l| l.group.clone()).unwrap_or_default(),
            header: loaded.map(|l| join_keys(&l.header)).unwrap_or_default(),
            loaded: loaded.cloned(),
            initial: None,
            reset: false,
        };
        form.initial = form.layout();
        form
    }

    /// Whether the form gives another layout than it did when the dialog opened.
    pub fn edited(&self) -> bool {
        self.reset || self.layout() != self.initial
    }

    /// The form cleared to the report's built-in layout, still against the layout it opened on.
    pub fn cleared(&self) -> Form {
        Form {
            loaded: self.loaded.clone(),
            initial: self.initial.clone(),
            reset: true,
            ..Form::new(self.kind, self.builtin.clone(), None)
        }
    }

    /// What the layout being edited names that the report or the title block does not have
    /// ([`wc_report::layout_problems`]), each without the report it is about. The form's
    /// built-in columns are checked against, so the report is not built for it.
    pub fn problems(&self, project: &wc_core::Project) -> Vec<String> {
        let Some(layout) = self.layout() else {
            return Vec::new();
        };
        wc_report::layout_problems_for(&self.builtin, &layout, project)
            .into_iter()
            .map(|p| p.split_once(": ").map_or(p.clone(), |(_, why)| why.to_owned()))
            .collect()
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
            header: split_keys(&self.header),
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

    /// The layout OK keeps for the report ([`Form::layout`]), or why it cannot be kept
    /// ([`Form::problem`]). The dialog and the assistant's `report_layouts` tool both keep a
    /// layout through this.
    pub fn kept(&self) -> Result<Option<ReportLayout>, String> {
        match self.problem() {
            Some(why) => Err(why),
            None => Ok(self.layout()),
        }
    }

    /// Move column `i` one place up (`-1`) or down (`1`).
    pub fn move_column(&mut self, i: usize, by: isize) {
        let len = self.columns.len();
        let Some(j) = i.checked_add_signed(by).filter(|j| i < len && *j < len) else {
            return;
        };
        self.columns.swap(i, j);
    }
}

/// Title-block keys as the form's text field shows them: comma separated, a key with a comma
/// or starting with a double quote (or with spaces around it) put in double quotes with each
/// quote in it doubled, so [`split_keys`] gives the same keys back.
pub fn join_keys(keys: &[String]) -> String {
    keys.iter()
        .map(|k| {
            if k.contains(',') || k.starts_with('"') || k.trim() != k {
                format!("\"{}\"", k.replace('"', "\"\""))
            } else {
                k.clone()
            }
        })
        .collect::<Vec<_>>()
        .join(", ")
}

/// The title-block keys typed in the form's text field ([`join_keys`]): split at commas
/// outside double quotes, trimmed, empty ones left out. A key in double quotes is kept as
/// written inside them, with `""` read as one quote.
pub fn split_keys(text: &str) -> Vec<String> {
    let mut keys = Vec::new();
    let mut chars = text.chars().peekable();
    loop {
        while chars.next_if(|c| c.is_whitespace()).is_some() {}
        let mut key = String::new();
        let quoted = chars.next_if_eq(&'"').is_some();
        if quoted {
            while let Some(c) = chars.next() {
                if c == '"' && chars.next_if_eq(&'"').is_none() {
                    break;
                }
                key.push(c);
            }
        }
        let mut rest = String::new();
        while let Some(c) = chars.next_if(|c| *c != ',') {
            rest.push(c);
        }
        if quoted {
            key.push_str(rest.trim_end());
        } else {
            key = rest.trim().to_owned();
        }
        if !key.is_empty() {
            keys.push(key);
        }
        if chars.next().is_none() {
            return keys;
        }
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
        .on_hover_text("Title-block values printed above the table on each page, by their keys, comma separated; put a key with a comma in double quotes");
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
    fn a_typed_group_column_or_header_key_that_names_nothing_is_shown() {
        let project = wc_core::Project::new("x");
        let (symbols, parts) = (Default::default(), Default::default());
        let inputs = wc_report::Inputs {
            project: &project,
            symbols: &symbols,
            parts: &parts,
        };
        let builtin = wc_report::build_builtin(wc_report::Kind::Parts, &inputs).columns;
        let mut f = Form::new(wc_report::Kind::Parts, builtin.clone(), None);
        assert!(
            f.problems(&project).is_empty(),
            "the built-in layout names nothing wrong"
        );
        f.group = "Maker".into();
        f.header = "project.name, revison".into();
        let p = f.problems(&project);
        assert_eq!(p.len(), 2, "{p:?}");
        assert!(p[0].starts_with("cannot group by \"Maker\""), "{p:?}");
        assert!(
            p[1].starts_with("header \"revison\" is not a title-block field"),
            "{p:?}"
        );
        f.group.clone_from(&builtin[0]);
        f.header = "project.name, revision".into();
        assert!(f.problems(&project).is_empty(), "{:?}", f.problems(&project));
    }

    #[test]
    fn a_header_key_with_a_comma_or_quote_survives_the_form() {
        let keys: Vec<String> = ["customer, site", "project.name", "5\" rack, \"A\"", "\"q", " pad "]
            .iter()
            .map(|s| (*s).to_owned())
            .collect();
        let l = ReportLayout {
            kind: "parts".into(),
            header: keys.clone(),
            ..ReportLayout::default()
        };
        let f = Form::new(wc_report::Kind::Parts, builtin(), Some(&l));
        assert_eq!(
            f.header,
            r#""customer, site", project.name, "5"" rack, ""A""", """q", " pad ""#
        );
        assert_eq!(f.layout().unwrap().header, keys);
        assert!(!f.edited());
        assert_eq!(split_keys(r#" "a,b" , c,, "" , d"#), ["a,b", "c", "d"]);
        assert_eq!(split_keys(r#""unclosed, x"#), ["unclosed, x"]);
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
        assert!(!again.edited());
        assert!(again.cleared().edited(), "back to the built-in layout is an edit");
        // A saved layout listing every built-in column as built in shows the same: untouched,
        // it is no edit although the form leaves the columns out.
        let all = ReportLayout {
            kind: "parts".into(),
            columns: builtin()
                .into_iter()
                .map(|from| ReportColumn { from, heading: None })
                .collect(),
            ..ReportLayout::default()
        };
        let f = Form::new(wc_report::Kind::Parts, builtin(), Some(&all));
        assert_eq!(f.layout(), None);
        assert!(!f.edited());
        // Pressing "Built-in layout" on it is an edit all the same: OK drops the saved one.
        assert!(f.cleared().edited());
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
        f.move_column(4, -1);
        f.move_column(usize::MAX, -1);
        assert_eq!(
            f.columns[0].from, "Colour",
            "moves past either end, or of no column, do nothing"
        );
    }
}
