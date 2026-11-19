//! The macro placeholders dialog: a project macro's placeholders (`{{name}}` in its texts)
//! and its value sets, one per variant ([`wc_core::macros`]), as a form. A placeholder
//! renamed here is renamed in the macro's texts too; saving checks the macro first.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use eframe::egui;
use wc_core::macros::{Macro, Placeholder, ValueSet, key};

/// One placeholder as the dialog edits it.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct PlaceholderRow {
    /// Its name in the file when the dialog opened, empty for one added here: renaming it
    /// renames its uses in the macro's texts.
    pub was: String,
    pub name: String,
    pub description: String,
    pub default: String,
}

/// One value set: a value for each placeholder, in the order of [`Form::placeholders`].
#[derive(Debug, Clone, PartialEq, Default)]
pub struct SetRow {
    pub name: String,
    pub description: String,
    /// Empty uses the placeholder's default.
    pub values: Vec<String>,
    /// Which values were stored as an empty value when the dialog opened: an empty value is
    /// a value (not the default), so one left empty is saved as it was.
    pub stored_empty: Vec<bool>,
    /// Values for names no placeholder declared when the dialog opened, kept so that saving
    /// reports them rather than dropping them.
    pub undeclared: BTreeMap<String, String>,
}

/// A project macro's placeholders and value sets, being edited.
#[derive(Debug, Clone, PartialEq)]
pub struct Form {
    pub path: PathBuf,
    /// The macro as read, the rest of which is saved unchanged.
    pub base: Macro,
    pub placeholders: Vec<PlaceholderRow>,
    pub sets: Vec<SetRow>,
}

impl Form {
    /// The form for the macro in `path`.
    pub fn open(path: &Path) -> Result<Form, String> {
        let base = wc_core::macros::read_macro(path).map_err(|e| e.to_string())?;
        let placeholders: Vec<PlaceholderRow> = base
            .placeholders
            .iter()
            .map(|p| PlaceholderRow {
                was: p.name.clone(),
                name: p.name.clone(),
                description: p.description.clone(),
                default: p.default.clone(),
            })
            .collect();
        let value = |v: &ValueSet, name: &str| {
            v.values
                .iter()
                .find(|(k, _)| key(k) == key(name))
                .map(|(_, v)| v.clone())
        };
        let sets = base
            .value_sets
            .iter()
            .map(|v| {
                let values: Vec<Option<String>> = placeholders.iter().map(|p| value(v, &p.name)).collect();
                SetRow {
                    name: v.name.clone(),
                    description: v.description.clone(),
                    stored_empty: values.iter().map(|v| v.as_deref() == Some("")).collect(),
                    values: values.into_iter().map(Option::unwrap_or_default).collect(),
                    undeclared: v
                        .values
                        .iter()
                        .filter(|(k, _)| !placeholders.iter().any(|p| key(&p.name) == key(k)))
                        .map(|(k, v)| (k.clone(), v.clone()))
                        .collect(),
                }
            })
            .collect();
        Ok(Form {
            path: path.to_owned(),
            base,
            placeholders,
            sets,
        })
    }

    /// Add a placeholder, with an empty value in every set.
    pub fn add_placeholder(&mut self) {
        self.placeholders.push(PlaceholderRow::default());
        for s in &mut self.sets {
            s.values.push(String::new());
            s.stored_empty.push(false);
        }
    }

    /// Remove placeholder `i` and its values.
    pub fn remove_placeholder(&mut self, i: usize) {
        if i < self.placeholders.len() {
            self.placeholders.remove(i);
            for s in &mut self.sets {
                if i < s.values.len() {
                    s.values.remove(i);
                }
                if i < s.stored_empty.len() {
                    s.stored_empty.remove(i);
                }
            }
        }
    }

    /// Add a value set, its values empty (the defaults).
    pub fn add_set(&mut self) {
        self.sets.push(SetRow {
            values: vec![String::new(); self.placeholders.len()],
            stored_empty: vec![false; self.placeholders.len()],
            ..SetRow::default()
        });
    }

    /// The macro the form describes: placeholders renamed in its texts, the value sets filled
    /// from the table (an empty cell keeps the default, unless it was stored empty). Checked
    /// with [`Macro::check`].
    pub fn to_macro(&self) -> Result<Macro, String> {
        let mut m = self.base.clone();
        let renames: Vec<(&str, &str)> = self
            .placeholders
            .iter()
            .filter(|p| !p.was.trim().is_empty() && !p.name.trim().is_empty() && p.was.trim() != p.name.trim())
            .map(|p| (p.was.as_str(), p.name.as_str()))
            .collect();
        m.rename_placeholders(&renames);
        m.placeholders = self
            .placeholders
            .iter()
            .map(|p| Placeholder {
                name: p.name.trim().to_owned(),
                description: p.description.trim().to_owned(),
                default: p.default.clone(),
            })
            .collect();
        let mut sets = Vec::new();
        for s in &self.sets {
            let mut values = s.undeclared.clone();
            for (i, (p, v)) in self.placeholders.iter().zip(&s.values).enumerate() {
                if v.is_empty() && !s.stored_empty.get(i).copied().unwrap_or(false) {
                    continue;
                }
                if p.name.trim().is_empty() {
                    return Err(format!(
                        "Value set {} has a value for a placeholder without a name.",
                        s.name.trim()
                    ));
                }
                values.insert(p.name.trim().to_owned(), v.clone());
            }
            sets.push(ValueSet {
                name: s.name.trim().to_owned(),
                description: s.description.trim().to_owned(),
                values,
            });
        }
        m.value_sets = sets;
        m.check().map_err(|e| e.to_string())?;
        Ok(m)
    }

    /// Check the macro and write it back to its file. A file changed since the dialog opened
    /// is not written over: the form stays as it is and the change is reported.
    pub fn save(&self) -> Result<PathBuf, String> {
        let m = self.to_macro()?;
        if wc_core::macros::read_macro(&self.path).ok().as_ref() != Some(&self.base) {
            return Err(format!(
                "{} was changed outside this dialog since it opened. Close the dialog and open it \
                 again to edit the file as it is now.",
                self.path.display()
            ));
        }
        let id = self
            .path
            .file_stem()
            .and_then(|s| s.to_str())
            .ok_or_else(|| format!("{} is not a macro file", self.path.display()))?;
        let root = self
            .path
            .parent()
            .and_then(Path::parent)
            .ok_or_else(|| format!("{} is not in a macros folder", self.path.display()))?;
        wc_core::macros::write_macro(root, id, &m).map_err(|e| e.to_string())
    }
}

fn field(ui: &mut egui::Ui, value: &mut String, width: f32, hint: &str) -> bool {
    ui.add(egui::TextEdit::singleline(value).desired_width(width).hint_text(hint))
        .changed()
}

/// Draw the form.
pub fn show(ui: &mut egui::Ui, form: &mut Form) {
    let used = form.base.used_placeholders();
    ui.strong("Placeholders");
    ui.label(format!(
        "Write {{{{name}}}} in a text of the macro, such as a description or a part number, and \
         declare it here. Used in its texts: {}.",
        if used.is_empty() {
            "none".to_owned()
        } else {
            used.join(", ")
        }
    ));
    let mut drop = None;
    egui::Grid::new("macro-placeholders")
        .num_columns(4)
        .spacing([6.0, 4.0])
        .show(ui, |ui| {
            for h in ["Name", "Description", "Default", ""] {
                ui.strong(h);
            }
            ui.end_row();
            for (i, p) in form.placeholders.iter_mut().enumerate() {
                field(ui, &mut p.name, 100.0, "kW");
                field(ui, &mut p.description, 180.0, "Motor power");
                field(ui, &mut p.default, 100.0, "");
                if ui.small_button("×").on_hover_text("Remove this placeholder").clicked() {
                    drop = Some(i);
                }
                ui.end_row();
            }
        });
    if let Some(i) = drop {
        form.remove_placeholder(i);
    }
    if ui.button("Add placeholder").clicked() {
        form.add_placeholder();
    }
    ui.separator();
    ui.strong("Value sets");
    ui.label("One row per variant. An empty cell uses the placeholder's default.");
    let mut drop = None;
    egui::ScrollArea::horizontal().id_salt("macro-sets").show(ui, |ui| {
        egui::Grid::new("macro-sets-grid")
            .num_columns(form.placeholders.len() + 3)
            .spacing([6.0, 4.0])
            .show(ui, |ui| {
                ui.strong("Name");
                ui.strong("Description");
                for p in &form.placeholders {
                    ui.strong(if p.name.trim().is_empty() { "?" } else { p.name.trim() });
                }
                ui.label("");
                ui.end_row();
                for (i, s) in form.sets.iter_mut().enumerate() {
                    field(ui, &mut s.name, 100.0, "3 kW");
                    field(ui, &mut s.description, 140.0, "");
                    s.values.resize(form.placeholders.len(), String::new());
                    s.stored_empty.resize(form.placeholders.len(), false);
                    for ((v, empty), p) in s.values.iter_mut().zip(&mut s.stored_empty).zip(&form.placeholders) {
                        // A value stored empty stays empty until it is edited; cleared, a cell
                        // uses the default.
                        let hint = if *empty { "(empty)" } else { p.default.as_str() };
                        if field(ui, v, 90.0, hint) {
                            *empty = false;
                        }
                    }
                    if ui.small_button("×").on_hover_text("Remove this value set").clicked() {
                        drop = Some(i);
                    }
                    ui.end_row();
                }
            });
    });
    if let Some(i) = drop {
        form.sets.remove(i);
    }
    if ui.button("Add value set").clicked() {
        form.add_set();
    }
}
