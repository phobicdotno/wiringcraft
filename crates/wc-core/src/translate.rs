//! Project texts in more than one language, through a translation dictionary.
//!
//! The project is drawn in one language, its source language. `translations.toml` in the
//! project folder lists each text with its translations:
//!
//! ```toml
//! source = "en"
//!
//! [[text]]
//! en = "Main switch"
//! nb = "Hovedbryter"
//!
//! [[text]]
//! en = "Pump 1"
//! nb = "Pumpe 1"
//! ```
//!
//! [`translate`] gives a copy of the project with its texts in another language (or in two,
//! such as `nb` with `en` under it), and lists the texts the dictionary has no translation
//! for. The texts it translates are page titles, device and location descriptions and free
//! texts on pages; tags, wire numbers, part numbers and title-block fields stay as they are.
//! A text is looked up as written, without its leading and trailing spaces. A text without a
//! word of two or more letters (such as `24 V`, `-K1` or `3~`) is never missing.

use std::collections::BTreeMap;
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::{Error, Project, Result, Shape};

/// The dictionary's file name in the project folder.
pub const TRANSLATIONS_FILE: &str = "translations.toml";

/// Put between the two languages of a text shown in two: `Hovedbryter / Main switch`.
pub const SEPARATOR: &str = " / ";

/// `translations.toml`: the project's source language and its translated texts.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct Dictionary {
    /// The language the project is drawn in, such as `en`. Empty means `en`.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub source: String,
    /// Languages to translate into before any text has a translation in them, so a language
    /// just added is kept: `languages = ["de"]`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub languages: Vec<String>,
    /// One entry per text: language code to text.
    #[serde(default, rename = "text", skip_serializing_if = "Vec::is_empty")]
    pub texts: Vec<BTreeMap<String, String>>,
}

/// A project text with no translation into the language asked for.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct Missing {
    /// The text in the source language.
    pub text: String,
    /// Where it is used, such as `page 3 title`, `-K1 description` or `page 3 text`.
    pub used: Vec<String>,
}

impl Dictionary {
    /// Read `translations.toml` from a project folder; a project without one has an empty
    /// dictionary.
    pub fn load(dir: &Path) -> Result<Self> {
        let path = dir.join(TRANSLATIONS_FILE);
        match std::fs::read_to_string(&path) {
            Ok(text) => Self::parse(&text).map_err(|message| Error::Toml { path, message }),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(e) => Err(Error::io(path, e)),
        }
    }

    /// Read a dictionary from its TOML text. Language codes are lowercased.
    pub fn parse(text: &str) -> std::result::Result<Self, String> {
        let mut d: Dictionary = toml::from_str(text).map_err(|e| e.message().to_owned())?;
        d.source = normalize(&d.source);
        d.languages = d.languages.iter().map(|l| normalize(l)).collect();
        for entry in &mut d.texts {
            *entry = std::mem::take(entry)
                .into_iter()
                .map(|(lang, text)| (normalize(&lang), text))
                .collect();
        }
        Ok(d)
    }

    /// The source language, `en` when the file does not say.
    pub fn source(&self) -> &str {
        if self.source.is_empty() { "en" } else { &self.source }
    }

    /// Every language the dictionary has a text in, the source language first.
    pub fn languages(&self) -> Vec<String> {
        let mut out = vec![self.source().to_owned()];
        for lang in self.languages.iter().chain(self.texts.iter().flat_map(|e| e.keys())) {
            if !out.contains(lang) {
                out.push(lang.clone());
            }
        }
        out
    }

    /// `text` (in the source language) in `lang`: itself for the source language, `None` when
    /// the dictionary has no non-empty translation.
    pub fn lookup(&self, text: &str, lang: &str) -> Option<String> {
        let lang = normalize(lang);
        let source = self.source();
        if lang == source {
            return Some(text.to_owned());
        }
        let key = text.trim();
        self.texts
            .iter()
            .filter(|e| e.get(source).is_some_and(|s| s.trim() == key))
            .find_map(|e| e.get(&lang).map(|t| t.trim()).filter(|t| !t.is_empty()))
            .map(str::to_owned)
    }

    /// Set the translation of `text` (in the source language) into `lang`; a blank one clears
    /// it. A text the dictionary has no entry for gets one. The source language is not changed.
    pub fn set(&mut self, text: &str, lang: &str, translation: &str) {
        let lang = normalize(lang);
        let source = self.source().to_owned();
        let (key, value) = (text.trim(), translation.trim());
        if lang.is_empty() || lang == source || key.is_empty() {
            return;
        }
        let matches = |e: &BTreeMap<String, String>| e.get(&source).is_some_and(|s| s.trim() == key);
        if value.is_empty() {
            // Every entry for the text, so a second one does not answer for it afterwards.
            for e in self.texts.iter_mut().filter(|e| matches(e)) {
                e.remove(&lang);
            }
            return;
        }
        match self.texts.iter_mut().find(|e| matches(e)) {
            Some(e) => {
                e.insert(lang, value.to_owned());
            }
            None => self
                .texts
                .push(BTreeMap::from([(source, key.to_owned()), (lang, value.to_owned())])),
        }
    }

    /// Write the dictionary to `translations.toml` in a project folder, as a whole (a failed
    /// write leaves the file as it was). Returns whether the file changed.
    pub fn save(&self, dir: &Path) -> Result<bool> {
        let path = dir.join(TRANSLATIONS_FILE);
        let text = toml::to_string_pretty(self).map_err(|e| Error::Toml {
            path: path.clone(),
            message: e.to_string(),
        })?;
        crate::store::write_if_changed(&path, &text)
    }
}

/// Set the translation of `text` (in the source language) into `lang` in the project folder's
/// `translations.toml`, or remove it when `translation` is blank. The other translations in
/// the file stay as they were. Returns whether the file changed. One language code is
/// required, and not the source language: its texts are the project's.
///
/// Calls made at the same time (the command line and the assistant, say) take turns: each
/// holds the project folder's [`TRANSLATIONS_LOCK`] from reading the file until its write is
/// in place, so the next one reads that write and keeps it. A program that writes the file
/// without that lock (an editor, say) and changes it after this call read it makes the call
/// fail with an error that asks to try again, and nothing is written then.
pub fn set_translation(dir: &Path, text: &str, lang: &str, translation: &str) -> Result<bool> {
    set_translation_checked(dir, text, lang, translation, || {}, || {})
}

/// The file next to `translations.toml` that [`set_translation`] holds locked while it reads
/// and writes the dictionary. Its name ends in `.tmp`, so a project's git repository leaves
/// it out.
pub const TRANSLATIONS_LOCK: &str = "translations.toml.lock.tmp";

/// Lock the project folder's [`TRANSLATIONS_LOCK`] for this process alone, waiting while
/// another holds it. The lock is let go when the returned file is closed.
fn lock_translations(dir: &Path) -> Result<std::fs::File> {
    std::fs::create_dir_all(dir).map_err(|e| Error::io(dir, e))?;
    let path = dir.join(TRANSLATIONS_LOCK);
    // A link left at the lock's name is not followed: whatever it points to stays untouched.
    if std::fs::symlink_metadata(&path).is_ok_and(|m| m.is_symlink()) {
        return Err(Error::Invalid(format!(
            "{} is a link; remove it and try again",
            path.display()
        )));
    }
    let mut open = std::fs::OpenOptions::new();
    open.append(true).create(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::custom_flags(&mut open, libc::O_NOFOLLOW);
    let file = open.open(&path).map_err(|e| Error::io(&path, e))?;
    file.lock().map_err(|e| Error::io(&path, e))?;
    Ok(file)
}

/// [`set_translation`], calling `meanwhile` between reading the file and checking it again,
/// and `checked` between that check and the write (for a test to change the file there).
fn set_translation_checked(
    dir: &Path,
    text: &str,
    lang: &str,
    translation: &str,
    meanwhile: impl FnOnce(),
    checked: impl FnOnce(),
) -> Result<bool> {
    let lang = match parse_languages(lang).as_deref() {
        Some([one]) => one.clone(),
        _ => {
            return Err(Error::Invalid(format!(
                "{:?} is not a language code: use one such as nb or de",
                lang.trim()
            )));
        }
    };
    if text.trim().is_empty() {
        return Err(Error::Invalid("no text to translate".into()));
    }
    // Held until this call returns, so no other call reads the file before this write is in.
    let _lock = lock_translations(dir)?;
    let path = dir.join(TRANSLATIONS_FILE);
    let read = || match std::fs::read_to_string(&path) {
        Ok(text) => Ok(Some(text)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(Error::io(&path, e)),
    };
    let before = read()?;
    let mut dict = match &before {
        Some(t) => Dictionary::parse(t).map_err(|message| Error::Toml {
            path: path.clone(),
            message,
        })?,
        None => Dictionary::default(),
    };
    if lang == dict.source() {
        return Err(Error::Invalid(format!(
            "{lang} is the source language: its texts are the project's own"
        )));
    }
    dict.set(text, &lang, translation);
    meanwhile();
    if read()? != before {
        return Err(Error::Invalid(format!(
            "{TRANSLATIONS_FILE} changed while the translation was being saved; nothing was written, try again"
        )));
    }
    checked();
    dict.save(dir)
}

/// A language code as compared: trimmed and lowercased.
fn normalize(lang: &str) -> String {
    lang.trim().to_ascii_lowercase()
}

/// Whether `text` has a word of two or more letters: one that can need a translation.
fn has_word(text: &str) -> bool {
    let mut run = 0;
    text.chars().any(|c| {
        run = if c.is_alphabetic() { run + 1 } else { 0 };
        run >= 2
    })
}

/// Parse a language choice: one code (`nb`), or two joined by `,` or `+` (`nb,en`) to show
/// each text in the first with the second after it. Returns `None` for an empty code or more
/// than two.
pub fn parse_languages(spec: &str) -> Option<Vec<String>> {
    let langs: Vec<String> = spec.split([',', '+']).map(normalize).collect();
    let ok = (1..=2).contains(&langs.len())
        && langs
            .iter()
            .all(|l| !l.is_empty() && l.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_'));
    ok.then_some(langs)
}

/// A copy of `project` with its texts in `langs` (one language, or two shown together), and
/// the texts that have no translation into one of them, sorted by text. A missing translation
/// keeps the source text in its place.
pub fn translate(project: &Project, dict: &Dictionary, langs: &[String]) -> (Project, Vec<Missing>) {
    let mut out = project.clone();
    let mut missing: BTreeMap<String, Vec<String>> = BTreeMap::new();
    each_text(&mut out, |text, used| {
        let mut parts = Vec::new();
        for lang in langs {
            match dict.lookup(text, lang) {
                Some(t) => parts.push(t),
                None => {
                    if has_word(text) {
                        let uses = missing.entry(text.trim().to_owned()).or_default();
                        if !uses.contains(&used) {
                            uses.push(used.clone());
                        }
                    }
                    parts.push(text.clone());
                }
            }
        }
        if parts.len() > 1 {
            parts = parts.iter().map(|p| p.trim().to_owned()).collect();
            parts.dedup();
        }
        *text = parts.join(SEPARATOR);
    });
    let missing = missing.into_iter().map(|(text, used)| Missing { text, used }).collect();
    (out, missing)
}

/// Every text of the project that can need a translation (one with a word of two letters),
/// trimmed, with where it is used, sorted by text: the rows of a translation table.
pub fn texts(project: &Project) -> Vec<Missing> {
    let mut found: BTreeMap<String, Vec<String>> = BTreeMap::new();
    each_text(&mut project.clone(), |text, used| {
        if has_word(text) {
            let uses = found.entry(text.trim().to_owned()).or_default();
            if !uses.contains(&used) {
                uses.push(used);
            }
        }
    });
    found.into_iter().map(|(text, used)| Missing { text, used }).collect()
}

/// Call `f` with each non-blank text [`translate`] translates and where it is used.
fn each_text(project: &mut Project, mut f: impl FnMut(&mut String, String)) {
    let mut tr = |text: &mut String, used: String| {
        if !text.trim().is_empty() {
            f(text, used);
        }
    };
    for page in &mut project.pages {
        tr(&mut page.title, format!("page {} title", page.number));
        for g in &mut page.graphics {
            if let Shape::Text { text, .. } = g {
                tr(text, format!("page {} text", page.number));
            }
        }
    }
    for (tag, device) in &mut project.devices {
        tr(&mut device.description, format!("{tag} description"));
    }
    for loc in &mut project.meta.locations {
        tr(&mut loc.description, format!("+{} description", loc.path));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Device, Location, Page, Point};

    type R<T = ()> = std::result::Result<T, Box<dyn std::error::Error>>;

    const DICT: &str = r#"
source = "EN"

[[text]]
en = "Main switch"
NB = "Hovedbryter"

[[text]]
en = "Pumps"
nb = "Pumper"

[[text]]
en = "Cabinet"
nb = ""
"#;

    fn project() -> R<Project> {
        let mut p = Project::new("demo");
        let mut page = Page::new("p1", "1", "Pumps");
        for t in ["Main switch ", "Spare", "24 V"] {
            page.graphics.push(Shape::Text {
                at: Point { x: 10.0, y: 10.0 },
                text: t.into(),
                height: 2.5,
                align: Default::default(),
                angle: 0.0,
            });
        }
        p.pages.push(page);
        p.devices.insert(
            "-Q1".parse()?,
            Device {
                description: "Main switch".into(),
                ..Default::default()
            },
        );
        p.meta.locations.push(Location {
            path: "L1".into(),
            description: "Cabinet".into(),
        });
        Ok(p)
    }

    fn texts(p: &Project) -> Vec<String> {
        let page = &p.pages[0];
        let mut out = vec![page.title.clone()];
        out.extend(page.graphics.iter().filter_map(|g| match g {
            Shape::Text { text, .. } => Some(text.clone()),
            _ => None,
        }));
        out.extend(p.devices.values().map(|d| d.description.clone()));
        out.extend(p.meta.locations.iter().map(|l| l.description.clone()));
        out
    }

    #[test]
    fn translates_titles_descriptions_and_free_texts_and_lists_what_is_missing() -> R {
        let d = Dictionary::parse(DICT)?;
        assert_eq!(d.source(), "en");
        assert_eq!(d.languages(), ["en", "nb"]);
        let (nb, missing) = translate(&project()?, &d, &["nb".into()]);
        assert_eq!(
            texts(&nb),
            ["Pumper", "Hovedbryter", "Spare", "24 V", "Hovedbryter", "Cabinet"]
        );
        assert_eq!(
            missing,
            [
                Missing {
                    text: "Cabinet".into(),
                    used: vec!["+L1 description".into()]
                },
                Missing {
                    text: "Spare".into(),
                    used: vec!["page 1 text".into()]
                },
            ]
        );
        Ok(())
    }

    #[test]
    fn the_source_language_is_unchanged_and_never_missing() -> R {
        let d = Dictionary::parse(DICT)?;
        let p = project()?;
        let (en, missing) = translate(&p, &d, &["en".into()]);
        assert_eq!(en, p);
        assert!(missing.is_empty());
        Ok(())
    }

    #[test]
    fn two_languages_show_both_and_a_text_without_translation_once() -> R {
        let d = Dictionary::parse(DICT)?;
        let (both, _) = translate(&project()?, &d, &parse_languages("nb,en").ok_or("languages")?);
        assert_eq!(
            texts(&both),
            [
                "Pumper / Pumps",
                "Hovedbryter / Main switch",
                "Spare",
                "24 V",
                "Hovedbryter / Main switch",
                "Cabinet"
            ]
        );
        Ok(())
    }

    #[test]
    fn parses_language_choices() -> R {
        assert_eq!(parse_languages("NB"), Some(vec!["nb".to_owned()]));
        assert_eq!(parse_languages("nb+en"), Some(vec!["nb".to_owned(), "en".to_owned()]));
        assert_eq!(parse_languages(""), None);
        assert_eq!(parse_languages("nb,en,de"), None);
        assert_eq!(parse_languages("n b"), None);
        Ok(())
    }

    #[test]
    fn a_project_without_a_dictionary_loads_an_empty_one_and_a_bad_file_is_an_error() -> R {
        let dir = tempfile::tempdir()?;
        assert_eq!(Dictionary::load(dir.path())?, Dictionary::default());
        std::fs::write(dir.path().join(TRANSLATIONS_FILE), "[[text]\n")?;
        assert!(matches!(Dictionary::load(dir.path()), Err(Error::Toml { .. })));
        Ok(())
    }

    #[test]
    fn lists_the_project_texts_and_saves_set_translations() -> R {
        let p = project()?;
        let rows: Vec<(String, Vec<String>)> = super::texts(&p).into_iter().map(|m| (m.text, m.used)).collect();
        assert_eq!(
            rows,
            [
                ("Cabinet".to_owned(), vec!["+L1 description".to_owned()]),
                (
                    "Main switch".to_owned(),
                    vec!["page 1 text".to_owned(), "-Q1 description".to_owned()]
                ),
                ("Pumps".to_owned(), vec!["page 1 title".to_owned()]),
                ("Spare".to_owned(), vec!["page 1 text".to_owned()]),
            ]
        );
        let mut d = Dictionary::parse(DICT)?;
        d.set("Spare ", "NB", " Reserve");
        d.set("Cabinet", "nb", "Skap");
        d.set("Pumps", "nb", "");
        d.set("Pumps", "en", "Changed");
        d.set("Nowhere", "de", "");
        let dir = tempfile::tempdir()?;
        assert!(d.save(dir.path())?);
        assert!(!d.save(dir.path())?);
        let again = Dictionary::load(dir.path())?;
        assert_eq!(again, d);
        assert_eq!(again.lookup("Spare", "nb").as_deref(), Some("Reserve"));
        assert_eq!(again.lookup("Cabinet", "nb").as_deref(), Some("Skap"));
        assert_eq!(again.lookup("Pumps", "nb"), None);
        assert_eq!(again.lookup("Pumps", "en").as_deref(), Some("Pumps"));
        assert_eq!(again.texts.len(), 4);
        Ok(())
    }

    #[test]
    fn set_translation_writes_and_removes_one_text_in_the_file() -> R {
        let dir = tempfile::tempdir()?;
        std::fs::write(dir.path().join(TRANSLATIONS_FILE), DICT)?;
        assert!(set_translation(dir.path(), "Spare", "NB", "Reserve")?);
        assert!(!set_translation(dir.path(), "Spare", "nb", "Reserve")?, "unchanged");
        assert!(set_translation(dir.path(), "Main switch", "nb", "")?);
        let d = Dictionary::load(dir.path())?;
        assert_eq!(d.lookup("Spare", "nb").as_deref(), Some("Reserve"));
        assert_eq!(d.lookup("Main switch", "nb"), None);
        assert_eq!(d.lookup("Pumps", "nb").as_deref(), Some("Pumper"), "others are kept");
        for (text, lang) in [("Spare", "en"), ("Spare", "nb,en"), ("Spare", ""), (" ", "nb")] {
            assert!(
                matches!(set_translation(dir.path(), text, lang, "x"), Err(Error::Invalid(_))),
                "{text:?} {lang:?}"
            );
        }
        // A project without a dictionary gets one.
        let empty = tempfile::tempdir()?;
        assert!(set_translation(empty.path(), "Pumps", "de", "Pumpen")?);
        assert_eq!(
            Dictionary::load(empty.path())?.lookup("Pumps", "de").as_deref(),
            Some("Pumpen")
        );
        Ok(())
    }

    #[test]
    fn set_translation_writes_nothing_when_the_file_changed_after_it_was_read() -> R {
        let dir = tempfile::tempdir()?;
        let path = dir.path().join(TRANSLATIONS_FILE);
        std::fs::write(&path, DICT)?;
        let other = format!("{DICT}\n[[text]]\nen = \"Spare\"\nde = \"Ersatz\"\n");
        let r = set_translation_checked(
            dir.path(),
            "Pumps",
            "nb",
            "Pumpene",
            || {
                let _ = std::fs::write(&path, &other);
            },
            || {},
        );
        assert!(matches!(r, Err(Error::Invalid(m)) if m.contains("try again")));
        // The other writer's file is left as it wrote it.
        assert_eq!(std::fs::read_to_string(&path)?, other);
        // Tried again, it is saved on top of the other change.
        assert!(set_translation(dir.path(), "Pumps", "nb", "Pumpene")?);
        let d = Dictionary::load(dir.path())?;
        assert_eq!(d.lookup("Pumps", "nb").as_deref(), Some("Pumpene"));
        assert_eq!(d.lookup("Spare", "de").as_deref(), Some("Ersatz"));
        // A file made meanwhile where there was none is a change too.
        let empty = tempfile::tempdir()?;
        let new = empty.path().join(TRANSLATIONS_FILE);
        let r = set_translation_checked(
            empty.path(),
            "Pumps",
            "nb",
            "Pumper",
            || {
                let _ = std::fs::write(&new, "source = \"en\"\n");
            },
            || {},
        );
        assert!(r.is_err());
        Ok(())
    }

    #[test]
    fn a_second_set_translation_waits_for_the_first_and_keeps_both_edits() -> R {
        let dir = tempfile::tempdir()?;
        std::fs::write(dir.path().join(TRANSLATIONS_FILE), DICT)?;
        let at = dir.path().to_owned();
        let mut other = None;
        // Another set starts after this one checked the file and before it writes: it has to
        // wait for this write instead of saving in between and being written over.
        let first = set_translation_checked(
            dir.path(),
            "Pumps",
            "nb",
            "Pumpene",
            || {},
            || {
                let t = std::thread::spawn(move || set_translation(&at, "Spare", "de", "Ersatz"));
                std::thread::sleep(std::time::Duration::from_millis(300));
                other = Some((t.is_finished(), t));
            },
        );
        assert!(first?);
        let Some((finished_early, t)) = other else {
            unreachable!("the check ran")
        };
        assert!(!finished_early, "the second set did not wait for the first");
        let Ok(second) = t.join() else {
            unreachable!("the second set panicked")
        };
        assert!(second?);
        let d = Dictionary::load(dir.path())?;
        assert_eq!(d.lookup("Pumps", "nb").as_deref(), Some("Pumpene"));
        assert_eq!(d.lookup("Spare", "de").as_deref(), Some("Ersatz"));
        Ok(())
    }

    #[test]
    fn clearing_a_translation_clears_it_from_every_entry_of_the_text_and_languages_are_kept() -> R {
        let mut d = Dictionary::parse(
            "source = \"en\"\nlanguages = [\"DE\"]\n[[text]]\nen = \"Pumps\"\nnb = \"Pumper\"\n[[text]]\nen = \"Pumps \"\nnb = \"Pumpene\"\n",
        )?;
        assert_eq!(d.languages(), ["en", "de", "nb"]);
        d.set("Pumps", "nb", "");
        assert_eq!(d.lookup("Pumps", "nb"), None);
        let dir = tempfile::tempdir()?;
        d.save(dir.path())?;
        // nb is gone with its last translation; de stays, declared.
        assert_eq!(Dictionary::load(dir.path())?.languages(), ["en", "de"]);
        Ok(())
    }
}
