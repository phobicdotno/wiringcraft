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
    /// The row whose cell has the keyboard: it stays shown under "Only missing" until the
    /// edit ends, though its cells are no longer blank.
    pub editing: Option<usize>,
    /// The languages added in this dialog. Only these are declared again on saving, so a
    /// column removed from the file meanwhile stays removed.
    added: Vec<String>,
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
            editing: None,
            added: Vec::new(),
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
        self.langs.push(lang.clone());
        self.added.push(lang);
        for r in &mut self.rows {
            r.cells.push(String::new());
            r.loaded.push(String::new());
        }
        self.new_lang.clear();
        Ok(())
    }

    /// Whether the form edits the dictionary of the project in `dir`.
    pub fn is_for(&self, dir: &Path) -> bool {
        self.dir == dir
    }

    /// How many texts have no translation into the language of column `col`.
    pub fn missing(&self, col: usize) -> usize {
        self.rows
            .iter()
            .filter(|r| r.cells.get(col).is_none_or(|c| c.trim().is_empty()))
            .count()
    }

    /// Whether row `i` is left out of the table: under "Only missing", a row with every
    /// translation, unless it is the one being typed into.
    pub fn hidden(&self, i: usize) -> bool {
        self.only_missing
            && self.editing != Some(i)
            && self
                .rows
                .get(i)
                .is_none_or(|r| r.cells.iter().all(|c| !c.trim().is_empty()))
    }

    /// Write the cells changed since the form opened into the project's `translations.toml`,
    /// read again first so translations changed there meanwhile are kept. Returns a line
    /// saying what was done.
    pub fn save(&self) -> Result<String, String> {
        self.save_locked(|| {})
    }

    /// [`Form::save`], calling `locked` once the translations lock is held and before the
    /// file is read (for a test to start another save there).
    fn save_locked(&self, locked: impl FnOnce()) -> Result<String, String> {
        // Held from the re-read until the save is in, like `translations set` from the command
        // line or the assistant: a set made at the same moment waits, or is read here.
        let _lock = wc_core::translate::lock_translations(&self.dir).map_err(|e| e.to_string())?;
        locked();
        let mut dict = Dictionary::load(&self.dir).map_err(|e| e.to_string())?;
        if dict.source() != self.source {
            // The rows are texts in the old source language: they would be filed under the
            // wrong one.
            return Err(format!(
                "the project's source language changed from {} to {} while the translations were open. Your edits are still here: close the dialog and open it again to translate from {}.",
                self.source,
                dict.source(),
                dict.source()
            ));
        }
        if dict.source.is_empty() {
            dict.source = self.source.clone();
        }
        // A language added with no translation yet is kept as a column.
        let known = dict.languages();
        let new_langs = self.added.iter().filter(|l| !known.contains(l)).count();
        if new_langs > 0 {
            // The columns named in the dialog's order, so they keep it when reopened. One
            // that was loaded but is gone from the file now was removed there: it stays gone.
            for l in &self.langs {
                let kept = known.contains(l) || self.added.contains(l);
                if kept && !dict.languages.contains(l) {
                    dict.languages.push(l.clone());
                }
            }
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
        if changed == 0 && new_langs == 0 {
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
    let hidden: Vec<bool> = (0..form.rows.len()).map(|i| form.hidden(i)).collect();
    if !form.langs.is_empty() && hidden.iter().all(|&h| h) {
        ui.label(egui::RichText::new("Every text has a translation into every language.").color(theme.faint));
    }
    let mut editing = None;
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
                for ((i, r), &hide) in form.rows.iter_mut().enumerate().zip(&hidden) {
                    if hide {
                        continue;
                    }
                    ui.label(&r.text).on_hover_text(r.used.join("\n"));
                    for (k, c) in r.cells.iter_mut().enumerate() {
                        let blank = c.trim().is_empty();
                        let mut edit = egui::TextEdit::singleline(c).id(cell_id(i, k)).desired_width(180.0);
                        if blank {
                            edit = edit.hint_text(egui::RichText::new("missing").color(theme.error));
                        }
                        if ui.add(edit).has_focus() {
                            editing = Some(i);
                        }
                    }
                    ui.end_row();
                }
            });
    });
    form.editing = editing;
    if let Some(e) = &form.error {
        ui.colored_label(theme.error, e);
    }
}

/// The id of the text field for row `row`, language column `col`.
pub fn cell_id(row: usize, col: usize) -> egui::Id {
    egui::Id::new(("translation-cell", row, col))
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use wc_core::translate::TRANSLATIONS_FILE;

    fn project() -> wc_core::Project {
        let mut p = wc_core::Project::new("x");
        p.pages.push(wc_core::Page::new("p1", "1", "Pumps"));
        p.pages.push(wc_core::Page::new("p2", "2", "Spare"));
        p
    }

    fn form(dict: &str) -> (tempfile::TempDir, Form) {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join(TRANSLATIONS_FILE), dict).unwrap();
        let f = Form::open(tmp.path(), &project()).unwrap();
        (tmp, f)
    }

    #[test]
    fn a_source_language_changed_meanwhile_keeps_the_draft_and_writes_nothing() {
        let (tmp, mut f) = form("source = \"en\"\n[[text]]\nen = \"Pumps\"\nnb = \"Pumper\"\n");
        f.rows.iter_mut().find(|r| r.text == "Spare").unwrap().cells[0] = "Reserve".into();
        let changed = "source = \"nb\"\n[[text]]\nnb = \"Pumper\"\nen = \"Pumps\"\n";
        std::fs::write(tmp.path().join(TRANSLATIONS_FILE), changed).unwrap();
        let err = f.save().unwrap_err();
        assert!(err.contains("changed from en to nb"), "{err}");
        assert_eq!(
            std::fs::read_to_string(tmp.path().join(TRANSLATIONS_FILE)).unwrap(),
            changed
        );
        assert_eq!(f.rows.iter().find(|r| r.text == "Spare").unwrap().cells[0], "Reserve");
    }

    #[test]
    fn a_set_translation_during_a_save_waits_and_both_edits_are_kept() {
        let (tmp, mut f) = form("source = \"en\"\n[[text]]\nen = \"Pumps\"\nnb = \"Pumper\"\n");
        let col = f.langs.iter().position(|l| l == "nb").unwrap();
        f.rows.iter_mut().find(|r| r.text == "Spare").unwrap().cells[col] = "Reserve".into();
        let at = tmp.path().to_owned();
        let mut other = None;
        // The command line sets another text while the form holds the lock and has not read
        // the file yet: it has to wait for the form's save, then keep the form's edit.
        let lock = tmp.path().join(wc_core::translate::TRANSLATIONS_LOCK);
        let saved = f.save_locked(|| {
            // The lock is held here: another handle on the lock file cannot take it.
            let held = std::fs::OpenOptions::new()
                .read(true)
                .append(true)
                .open(&lock)
                .map(|f| matches!(f.try_lock(), Err(std::fs::TryLockError::WouldBlock)));
            assert!(matches!(held, Ok(true)), "the lock is not held: {held:?}");
            let t = std::thread::spawn(move || wc_core::translate::set_translation(&at, "Pumps", "de", "Pumpen"));
            std::thread::sleep(std::time::Duration::from_millis(300));
            other = Some((t.is_finished(), t));
        });
        assert!(saved.unwrap().starts_with("Saved 1 translation"));
        let (finished_early, t) = other.unwrap();
        assert!(!finished_early, "the set did not wait for the form's save");
        assert!(t.join().unwrap().unwrap());
        let d = wc_core::translate::Dictionary::load(tmp.path()).unwrap();
        assert_eq!(d.lookup("Spare", "nb").as_deref(), Some("Reserve"));
        assert_eq!(d.lookup("Pumps", "de").as_deref(), Some("Pumpen"));
        assert_eq!(d.lookup("Pumps", "nb").as_deref(), Some("Pumper"));
    }

    #[test]
    fn a_language_added_without_translations_is_kept() {
        let (tmp, mut f) = form("source = \"en\"\n");
        f.new_lang = "de".into();
        f.add_language().unwrap();
        assert!(f.save().unwrap().starts_with("Saved"));
        let again = Form::open(tmp.path(), &project()).unwrap();
        assert_eq!(again.langs, ["de"]);
        assert_eq!(again.missing(0), 2);
    }

    #[test]
    fn a_language_removed_meanwhile_is_not_declared_again() {
        let (tmp, f) = form("source = \"en\"\nlanguages = [\"nb\"]\n");
        assert_eq!(f.langs, ["nb"]);
        let removed = "source = \"en\"\n";
        std::fs::write(tmp.path().join(TRANSLATIONS_FILE), removed).unwrap();
        assert_eq!(f.save().unwrap(), "No translation was changed.");
        assert_eq!(
            std::fs::read_to_string(tmp.path().join(TRANSLATIONS_FILE)).unwrap(),
            removed
        );

        // Adding another language keeps the new one but still not the removed one.
        let mut f = f;
        f.new_lang = "de".into();
        f.add_language().unwrap();
        f.save().unwrap();
        let again = Form::open(tmp.path(), &project()).unwrap();
        assert_eq!(again.langs, ["de"]);
    }

    #[test]
    fn only_missing_hides_the_translated_rows_but_not_the_one_being_typed_into() {
        let (_tmp, mut f) = form("source = \"en\"\n[[text]]\nen = \"Pumps\"\nnb = \"Pumper\"\n");
        let pumps = f.rows.iter().position(|r| r.text == "Pumps").unwrap();
        let spare = f.rows.iter().position(|r| r.text == "Spare").unwrap();
        assert!(!f.hidden(pumps), "everything is shown without the filter");
        f.only_missing = true;
        assert!(f.hidden(pumps) && !f.hidden(spare));
        f.rows[spare].cells[0] = "Reserve".into();
        assert!(f.hidden(spare), "every row is translated: the table is empty");
        f.editing = Some(spare);
        assert!(!f.hidden(spare));
    }

    /// The texts `show` paints for `form` in one frame.
    fn painted(form: &mut Form) -> Vec<String> {
        let ctx = egui::Context::default();
        let out = ctx.run(egui::RawInput::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| show(ui, form));
        });
        out.shapes
            .iter()
            .filter_map(|c| match &c.shape {
                egui::Shape::Text(t) => Some(t.galley.text().to_owned()),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn only_missing_says_so_when_it_hides_every_row() {
        const DONE: &str = "Every text has a translation into every language.";
        let (_tmp, mut f) = form("source = \"en\"\n[[text]]\nen = \"Pumps\"\nnb = \"Pumper\"\n");
        f.only_missing = true;
        assert!(!painted(&mut f).iter().any(|t| t == DONE), "Spare still misses one");
        let spare = f.rows.iter().position(|r| r.text == "Spare").unwrap();
        f.rows[spare].cells[0] = "Reserve".into();
        assert!(painted(&mut f).iter().any(|t| t == DONE), "every row is hidden");
        // Without the filter nothing is hidden, so there is nothing to explain.
        f.only_missing = false;
        assert!(!painted(&mut f).iter().any(|t| t == DONE));
    }

    #[test]
    fn a_row_being_typed_into_stays_shown_under_only_missing() {
        let (_tmp, mut f) = form("source = \"en\"\nlanguages = [\"nb\"]\n");
        f.only_missing = true;
        let row = f.rows.iter().position(|r| r.text == "Spare").unwrap();
        let ctx = egui::Context::default();
        let frame = |f: &mut Form, events: Vec<egui::Event>| {
            let input = egui::RawInput {
                events,
                ..Default::default()
            };
            let _ = ctx.run(input, |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| show(ui, f));
            });
        };
        frame(&mut f, vec![]);
        ctx.memory_mut(|m| m.request_focus(cell_id(row, 0)));
        frame(&mut f, vec![]);
        for t in ["R", "e", "serve"] {
            frame(&mut f, vec![egui::Event::Text(t.into())]);
        }
        assert_eq!(f.rows[row].cells[0], "Reserve");
        // Once the edit ends, the row is filtered out like any other with every translation.
        ctx.memory_mut(|m| m.surrender_focus(cell_id(row, 0)));
        frame(&mut f, vec![]);
        assert_eq!(f.editing, None);
    }
}
