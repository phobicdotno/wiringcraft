//! The translations dialog: the project's texts ([`wc_core::translate::texts`]) as rows with a
//! column per language of `translations.toml`, missing translations marked, saved to the file.

use std::path::{Path, PathBuf};

use eframe::egui;
use wc_core::translate::{Dictionary, parse_languages, texts};

use crate::theme::Theme;

/// One project text and its translations, one cell per language of [`Form::langs`].
#[derive(Debug, Clone, PartialEq)]
pub struct Row {
    /// The text as drawn, in the source language.
    pub text: String,
    /// Where it is used, such as `page 3 title`.
    pub used: Vec<String>,
    pub cells: Vec<String>,
    /// The cells as loaded: only the ones changed since are written, so a translation edited
    /// in the file meanwhile is kept.
    loaded: Vec<String>,
}

/// The dialog's state.
#[derive(Debug, Clone, PartialEq)]
pub struct Form {
    /// The project folder the dictionary is in.
    pub dir: PathBuf,
    /// The source language, the one the project is drawn in.
    pub source: String,
    /// The other languages, one column each.
    pub langs: Vec<String>,
    pub rows: Vec<Row>,
    /// The code typed for a language to add.
    pub new_lang: String,
    /// Show only the texts missing a translation.
    pub only_missing: bool,
    pub error: Option<String>,
}

impl Form {
    /// The form for the project in `dir`: its texts with their translations from the
    /// project's `translations.toml`.
    pub fn open(dir: &Path, project: &wc_core::Project) -> Result<Self, String> {
        let dict = Dictionary::load(dir).map_err(|e| e.to_string())?;
        let langs: Vec<String> = dict.languages().into_iter().skip(1).collect();
        let rows = texts(project)
            .into_iter()
            .map(|t| {
                let cells: Vec<String> = langs
                    .iter()
                    .map(|l| dict.lookup(&t.text, l).unwrap_or_default())
                    .collect();
                Row {
                    text: t.text,
                    used: t.used,
                    loaded: cells.clone(),
                    cells,
                }
            })
            .collect();
        Ok(Form {
            dir: dir.to_path_buf(),
            source: dict.source().to_owned(),
            langs,
            rows,
            new_lang: String::new(),
            only_missing: false,
            error: None,
        })
    }

    /// Add the language typed in [`Form::new_lang`] as a new, empty column.
    pub fn add_language(&mut self) -> Result<(), String> {
        let lang = match parse_languages(&self.new_lang).as_deref() {
            Some([one]) => one.clone(),
            _ => {
                return Err(format!(
                    "{:?} is not a language code: use one such as nb or de.",
                    self.new_lang.trim()
                ));
            }
        };
        if lang == self.source || self.langs.contains(&lang) {
            return Err(format!("{lang} is already a column."));
        }
        self.langs.push(lang);
        for r in &mut self.rows {
            r.cells.push(String::new());
            r.loaded.push(String::new());
        }
        self.new_lang.clear();
        Ok(())
    }

    /// How many texts have no translation into the language of column `col`.
    pub fn missing(&self, col: usize) -> usize {
        self.rows
            .iter()
            .filter(|r| r.cells.get(col).is_none_or(|c| c.trim().is_empty()))
            .count()
    }

    /// Write the cells changed since the form opened into the project's `translations.toml`,
    /// read again first so translations changed there meanwhile are kept. Returns a line
    /// saying what was done.
    pub fn save(&self) -> Result<String, String> {
        let mut dict = Dictionary::load(&self.dir).map_err(|e| e.to_string())?;
        if dict.source.is_empty() {
            dict.source = self.source.clone();
        }
        let mut changed = 0;
        for r in &self.rows {
            for (k, lang) in self.langs.iter().enumerate() {
                let (cell, was) = (r.cells.get(k), r.loaded.get(k));
                if let Some(cell) = cell.filter(|c| c.trim() != was.map_or("", |w| w.trim())) {
                    dict.set(&r.text, lang, cell);
                    changed += 1;
                }
            }
        }
        if changed == 0 {
            return Ok("No translation was changed.".into());
        }
        dict.save(&self.dir).map_err(|e| e.to_string())?;
        Ok(format!(
            "Saved {changed} translation{} to {}.",
            if changed == 1 { "" } else { "s" },
            wc_core::translate::TRANSLATIONS_FILE
        ))
    }
}

/// The form: a language bar, then a table of texts with a column per language. Empty cells
/// are marked as missing.
pub fn show(ui: &mut egui::Ui, form: &mut Form) {
    let theme = Theme::of(ui.ctx());
    ui.horizontal(|ui| {
        ui.label(format!("Drawn in: {}", form.source));
        ui.separator();
        ui.label("Add language");
        let typed = ui.add(
            egui::TextEdit::singleline(&mut form.new_lang)
                .desired_width(60.0)
                .hint_text("nb"),
        );
        let enter = typed.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
        if ui.button("Add").clicked() || enter {
            form.error = form.add_language().err();
        }
        ui.separator();
        ui.checkbox(&mut form.only_missing, "Only missing");
    });
    if !form.langs.is_empty() {
        let counts: Vec<String> = (0..form.langs.len())
            .map(|k| format!("{}: {} missing", form.langs[k], form.missing(k)))
            .collect();
        ui.label(egui::RichText::new(counts.join(", ")).color(theme.faint));
    }
    if form.rows.is_empty() {
        ui.label("The project has no texts to translate yet.");
        return;
    }
    if form.langs.is_empty() {
        ui.label("Add a language to translate the texts into.");
    }
    let only_missing = form.only_missing;
    egui::ScrollArea::both().max_height(420.0).show(ui, |ui| {
        egui::Grid::new("translations")
            .num_columns(form.langs.len() + 1)
            .striped(true)
            .spacing([8.0, 4.0])
            .show(ui, |ui| {
                ui.label(egui::RichText::new(&form.source).strong());
                for l in &form.langs {
                    ui.label(egui::RichText::new(l).strong());
                }
                ui.end_row();
                for r in &mut form.rows {
                    if only_missing && r.cells.iter().all(|c| !c.trim().is_empty()) {
                        continue;
                    }
                    ui.label(&r.text).on_hover_text(r.used.join("\n"));
                    for c in &mut r.cells {
                        let blank = c.trim().is_empty();
                        let mut edit = egui::TextEdit::singleline(c).desired_width(180.0);
                        if blank {
                            edit = edit.hint_text(egui::RichText::new("missing").color(theme.error));
                        }
                        ui.add(edit);
                    }
                    ui.end_row();
                }
            });
    });
    if let Some(e) = &form.error {
        ui.colored_label(theme.error, e);
    }
}
