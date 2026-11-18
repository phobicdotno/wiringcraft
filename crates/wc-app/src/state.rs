//! The open project and everything the user is doing with it, independent of drawing.

use eframe::egui;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use wc_core::geom::{Point, Rotation};
use wc_core::lock::ItemKind;
use wc_core::project::PageSize;
use wc_core::route::{self, Axis, Route};
use wc_core::template::PageTemplate;
use wc_core::{DevicePart, DeviceTag, Endpoint, Page, Part, PlacedSymbol, Project, Symbol, Wire, store};

/// How a part is listed in the picker: manufacturer, order number and description.
pub fn part_label(p: &Part) -> String {
    let head: Vec<&str> = [p.manufacturer.as_str(), p.order_number.as_str()]
        .into_iter()
        .filter(|s| !s.is_empty())
        .collect();
    match (head.is_empty(), p.description.is_empty()) {
        (true, true) => p.id.clone(),
        (true, false) => p.description.clone(),
        (false, true) => head.join(" "),
        (false, false) => format!("{} \u{2013} {}", head.join(" "), p.description),
    }
}

/// What a click on the canvas does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tool {
    Select,
    Place,
    Wire,
    Text,
    Line,
    Rect,
    Circle,
    Arc,
    Polyline,
    Comment,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Selection {
    Symbol(String),
    Wire(String),
    /// A free drawing on the page (line, rectangle, circle, text), by its index in `graphics`.
    Graphic(usize),
}

/// One part of a wire's spec, edited on its own.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SpecPart {
    Section,
    Colour,
    Kind,
}

impl SpecPart {
    pub const ALL: [SpecPart; 3] = [SpecPart::Section, SpecPart::Colour, SpecPart::Kind];

    pub fn label(self) -> &'static str {
        match self {
            SpecPart::Section => "Cross-section",
            SpecPart::Colour => "Colour",
            SpecPart::Kind => "Wire type",
        }
    }

    pub fn hint(self) -> &'static str {
        match self {
            SpecPart::Section => "e.g. 1.5 mm²",
            SpecPart::Colour => "e.g. BU",
            SpecPart::Kind => "e.g. H07V-K",
        }
    }

    pub fn of(self, s: &wc_core::WireSpec) -> &Option<String> {
        match self {
            SpecPart::Section => &s.section,
            SpecPart::Colour => &s.colour,
            SpecPart::Kind => &s.kind,
        }
    }

    pub fn of_mut(self, s: &mut wc_core::WireSpec) -> &mut Option<String> {
        match self {
            SpecPart::Section => &mut s.section,
            SpecPart::Colour => &mut s.colour,
            SpecPart::Kind => &mut s.kind,
        }
    }
}

/// A macro found in the project or a library source.
#[derive(Debug, Clone)]
pub struct MacroEntry {
    /// `project` or the library source's name.
    pub source: String,
    pub path: PathBuf,
    pub name: String,
    pub description: String,
    /// Why the file could not be read, if it could not.
    pub error: Option<String>,
}

/// What [`Open::number_wires`] did.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct WireNumbering {
    pub done: wc_core::numbering::Numbered,
    /// Numbers of the pages left alone because someone else has them open.
    pub skipped: Vec<String>,
}

impl WireNumbering {
    /// One line for the person who asked.
    pub fn report(&self) -> String {
        let mut s = format!(
            "Numbered {} connection(s); {} wire(s) changed. Existing numbers were kept.",
            self.done.connections, self.done.wires
        );
        if !self.skipped.is_empty() {
            s.push_str(&format!(
                " Page(s) {} were left as they are: someone else has them open. Number again once they are free.",
                self.skipped.join(", ")
            ));
        }
        s
    }
}

/// What [`Open::back_to`] goes back to, or [`Open::keep`] builds on.
pub(crate) struct Mark {
    project: Project,
    symbols: std::collections::BTreeSet<String>,
    parts: std::collections::BTreeSet<String>,
    undo: Vec<Project>,
    redo: Vec<Project>,
    undo_dropped: usize,
    history: u64,
    form_step: Option<u64>,
    dirty: bool,
    terminals_before: Option<wc_core::project::TerminalNumbers>,
    /// The vendored symbol and part files there were, or why that is not known.
    files: Result<std::collections::BTreeSet<(ItemKind, String)>, String>,
}

/// The vendored symbol and part files by kind and id (plain files only, not links), from folders
/// inside the project folder. Only names are read. An error (with its reason) when a folder that
/// may be there cannot be checked or listed, or lies outside the project: then nothing is known
/// to be new.
fn vendor_names(dir: &Path) -> Result<std::collections::BTreeSet<(ItemKind, String)>, String> {
    let root = dir.canonicalize().map_err(|e| format!("{}: {e}", dir.display()))?;
    let mut out = std::collections::BTreeSet::new();
    for kind in [ItemKind::Symbol, ItemKind::Part] {
        let folder = dir.join(store::VENDOR_DIR).join(kind.dir());
        let at = |e: std::io::Error| format!("{}: {e}", folder.display());
        // Only a folder known to be missing is skipped; one that cannot be checked is unknown.
        if !folder.try_exists().map_err(at)? {
            continue;
        }
        let real = folder.canonicalize().map_err(at)?;
        if !real.starts_with(&root) {
            return Err(format!("{} lies outside the project", folder.display()));
        }
        for e in std::fs::read_dir(real).map_err(at)? {
            let e = e.map_err(at)?;
            let path = e.path();
            if e.file_type().map_err(at)?.is_file()
                && path.extension().is_some_and(|x| x == "json")
                && let Some(id) = path.file_stem().and_then(|s| s.to_str())
            {
                out.insert((kind, id.to_owned()));
            }
        }
    }
    Ok(out)
}

pub struct Open {
    pub dir: PathBuf,
    pub project: Project,
    /// The project's vendored symbols.
    pub symbols: BTreeMap<String, Symbol>,
    /// Ids of every symbol in the project's library sources (for the picker).
    pub library: Vec<(String, String)>,
    /// The project's vendored parts that devices use.
    pub parts: BTreeMap<String, Part>,
    /// Every part in the project and its library sources, as (id, label), for the part picker.
    pub part_library: Vec<(String, String)>,
    pub dirty: bool,
    undo: Vec<Project>,
    redo: Vec<Project>,
    /// Snapshots dropped from the front of `undo` because it was full, so marks stay valid.
    undo_dropped: usize,
    /// Counts every change to the undo history (checkpoint, undo, redo, squash).
    history: u64,
    /// The `history` count right after a properties form made its own undo step, so later
    /// typing in the form joins that step only while nothing else has touched the history.
    form_step: Option<u64>,
    pub branch: String,
    /// Underlay pictures loaded as textures, by file; `None` when the file cannot be shown.
    pub underlays: BTreeMap<String, Option<egui::TextureHandle>>,
    /// Why a picture in `underlays` has no texture, by file: missing, unreadable, outside the
    /// project, or not a picture.
    pub picture_problems: BTreeMap<String, String>,
    /// The terminal numbering before the change in progress (taken at its checkpoint), so
    /// jumpers can follow their terminals once it is done.
    terminals_before: Option<wc_core::project::TerminalNumbers>,
    /// In a shared session, the pages someone else has open (page id to their name). Tools that
    /// work on every page leave these alone; the session would refuse the change otherwise.
    pub held_by_others: BTreeMap<String, String>,
    /// Every version of a vendored symbol this window has loaded, by content hash, so Undo
    /// and Redo can put back the version the restored lock names.
    symbol_versions: BTreeMap<String, Symbol>,
    /// Symbols whose vendored file may hold another version than the one loaded, after a
    /// failed Undo or Redo (or a command's result) could not be put back. The next Undo, Redo
    /// or save writes them again.
    stale_symbol_files: std::collections::BTreeSet<String>,
    /// Tests: whether each coming symbol file write fails, in order.
    #[cfg(test)]
    pub(crate) fail_symbol_writes: std::collections::VecDeque<bool>,
    /// Tests: a file that turns into a folder once the update's backups are taken, so the
    /// update's write of it fails after other items were already written.
    #[cfg(test)]
    block_after_snapshot: Option<PathBuf>,
    /// Tests: a file whose backup read fails, though the file is a plain readable one.
    #[cfg(test)]
    unreadable: Option<PathBuf>,
    /// Tests: a library file rewritten once the update's backups are taken, as if someone
    /// saved it in between.
    #[cfg(test)]
    rewrite_after_snapshot: Option<(PathBuf, String)>,
    /// Why the last Undo or Redo was not done, when it failed.
    undo_error: Option<String>,
    /// Parts the last Undo or Redo could not load, with the reason, though the step was done.
    step_warning: Option<String>,
}

/// The most Undo steps kept; older ones are dropped.
const UNDO_STEPS: usize = 200;

/// The history as it was before a library update, so a failed one leaves no trace in it.
struct HistoryState {
    redo: Vec<Project>,
    /// The oldest Undo step, when the update's checkpoint pushes it out.
    evicted: Option<Project>,
    dirty: bool,
    history: u64,
}

impl Open {
    pub fn load(dir: &Path) -> Result<Self, String> {
        let project = store::load(dir).map_err(|e| e.to_string())?;
        let (symbols, _missing) = store::vendored_symbols(dir, &project);
        let mut me = Self {
            dir: dir.to_path_buf(),
            project,
            symbols,
            library: Vec::new(),
            parts: BTreeMap::new(),
            part_library: Vec::new(),
            dirty: false,
            undo: Vec::new(),
            redo: Vec::new(),
            undo_dropped: 0,
            history: 0,
            form_step: None,
            branch: wc_vcs::Git::new(dir).current_branch().unwrap_or_default(),
            underlays: BTreeMap::new(),
            picture_problems: BTreeMap::new(),
            terminals_before: None,
            held_by_others: BTreeMap::new(),
            symbol_versions: BTreeMap::new(),
            stale_symbol_files: Default::default(),
            #[cfg(test)]
            fail_symbol_writes: Default::default(),
            #[cfg(test)]
            block_after_snapshot: None,
            #[cfg(test)]
            unreadable: None,
            #[cfg(test)]
            rewrite_after_snapshot: None,
            undo_error: None,
            step_warning: None,
        };
        // Locked symbols no page uses are loaded too, so Undo can put back the version a
        // library update replaces.
        me.reload_symbols();
        me.refresh_library();
        Ok(me)
    }

    /// The texture for a page's underlay picture, loaded on first use. Pictures larger than the
    /// graphics card takes are scaled down to fit.
    pub fn underlay_texture(&mut self, ctx: &egui::Context, u: &wc_core::Underlay) -> Option<egui::TextureHandle> {
        self.picture_texture(ctx, &u.file)
    }

    /// The texture for a picture file in the project (an underlay or the logo), loaded on first
    /// use; `None` when it is missing, outside the project or not a picture, with the reason in
    /// [`Open::picture_problems`].
    pub fn picture_texture(&mut self, ctx: &egui::Context, file: &str) -> Option<egui::TextureHandle> {
        if let Some(t) = self.underlays.get(file) {
            return t.clone();
        }
        let max_side = ctx.input(|i| i.max_texture_side).max(1) as u32;
        let img = match wc_pdf::read_picture(&self.dir, file) {
            Ok(Some(bytes)) => image::load_from_memory(&bytes).map_err(|e| format!("not a picture: {e}")),
            Ok(None) => Err("the file is missing".to_owned()),
            Err(why) => Err(why),
        };
        let tex = match img {
            Ok(img) => {
                self.picture_problems.remove(file);
                let img = if img.width() > max_side || img.height() > max_side {
                    img.resize(max_side, max_side, image::imageops::FilterType::Triangle)
                } else {
                    img
                };
                let rgba = img.to_rgba8();
                let size = [rgba.width() as usize, rgba.height() as usize];
                let pixels = egui::ColorImage::from_rgba_unmultiplied(size, rgba.as_raw());
                Some(ctx.load_texture(format!("picture:{file}"), pixels, egui::TextureOptions::LINEAR))
            }
            Err(why) => {
                self.picture_problems.insert(file.to_owned(), why);
                None
            }
        };
        self.underlays.insert(file.to_owned(), tex.clone());
        tex
    }

    /// Copy the picture at `src` into the project's `pictures` folder and make it the project
    /// logo, as one undo step. A different file of the same name already there is kept and the
    /// copy gets a new name, so an undo can still show the old logo. Returns the project path.
    pub fn set_logo_from(&mut self, src: &Path) -> Result<String, String> {
        let file = wc_core::Logo::copy_into(&self.dir, src)?;
        // A picture shown before under this name may have been another one.
        self.underlays.remove(&file);
        let cover = self.project.meta.logo.as_ref().and_then(|l| l.cover);
        self.checkpoint();
        self.project.meta.logo = Some(wc_core::Logo {
            file: file.clone(),
            cover,
        });
        Ok(file)
    }

    /// Take the logo off the cover and the title blocks, as one undo step. Its file stays in the
    /// project, so an undo brings it back.
    pub fn remove_logo(&mut self) {
        if self.project.meta.logo.is_some() {
            self.checkpoint();
            self.project.meta.logo = None;
        }
    }

    /// Put the logo's box on the cover page at `min`..`max`. The caller takes the undo
    /// checkpoint, once for a whole drag.
    pub fn set_logo_place(&mut self, min: Point, max: Point) {
        if let Some(logo) = self.project.meta.logo.as_mut() {
            logo.cover = Some((min, max));
            self.dirty = true;
        }
    }

    pub fn refresh_library(&mut self) {
        let mut items: BTreeMap<String, String> =
            self.symbols.iter().map(|(k, s)| (k.clone(), s.name.clone())).collect();
        if let Ok(lib) = wc_library::Library::open(&self.dir, &self.project) {
            for id in lib.index(ItemKind::Symbol).into_keys() {
                if !items.contains_key(&id)
                    && let Ok((s, _)) = lib.symbol(&id)
                {
                    items.insert(id, s.name);
                }
            }
        }
        self.library = items.into_iter().collect();

        self.parts = self
            .project
            .used_parts()
            .into_iter()
            .filter_map(|id| store::read_vendored_part(&self.dir, &id).ok().map(|p| (id, p)))
            .collect();
        let mut parts: BTreeMap<String, String> = self.parts.iter().map(|(k, p)| (k.clone(), part_label(p))).collect();
        if let Ok(lib) = wc_library::Library::open(&self.dir, &self.project) {
            for id in lib.index(ItemKind::Part).into_keys() {
                if !parts.contains_key(&id)
                    && let Ok((p, _)) = lib.part(&id)
                {
                    parts.insert(id, part_label(&p));
                }
            }
        }
        self.part_library = parts.into_iter().collect();
    }

    /// Give a device a part (taken from the library into the project when needed), or none.
    pub fn set_device_part(&mut self, tag: &DeviceTag, part: Option<&str>) -> Result<(), String> {
        let current = self.project.devices.get(tag).and_then(|d| d.part.as_deref());
        if current == part {
            return Ok(());
        }
        let lock = self.project.lock.clone();
        if let Some(id) = part
            && let Err(e) = self.ensure_part(id)
        {
            self.project.lock = lock;
            return Err(e);
        }
        self.checkpoint_with_lock(lock);
        self.project.devices.entry(tag.clone()).or_default().part = part.map(str::to_owned);
        Ok(())
    }

    /// Write the articles of a list that came back from the WAGO configurator onto the terminal
    /// and strip devices and the jumpers, as one undo step (see [`wc_import::wago`]). Articles are looked up
    /// among the project's own parts, then the libraries'; a library part that is used is taken
    /// into the project. Returns the report: what changed and every row not matched, with why.
    pub fn import_wago_configured(&mut self, bytes: &[u8]) -> Result<String, String> {
        let (rows, mut warnings) = wc_import::wago::read_configured(bytes).map_err(|e| e.to_string())?;
        let (candidates, more) = wc_import::wago::Candidates::load(&self.dir, &self.project);
        warnings.extend(more);
        // Jumpers follow a terminal change still in progress first, so the rows are matched to
        // the jumpers as they will be.
        self.follow_terminals();
        let plan = wc_import::wago::plan(
            &wc_report::Inputs {
                project: &self.project,
                symbols: &self.symbols,
                parts: &candidates.all(),
            },
            &candidates,
            &rows,
        );
        let mut unmatched = plan.unmatched.clone();
        let mut taken = plan.clone();
        // The lock as it was: the undo step goes back to it, and a part that cannot be used
        // loses the pin it may have got on the way.
        let lock = self.project.lock.clone();
        for id in plan.parts() {
            let r = candidates
                .take(&self.dir, &mut self.project, id)
                .and_then(|()| self.ensure_part(id));
            if let Err(why) = r {
                match lock.parts.get(id) {
                    Some(e) => {
                        self.project.lock.parts.insert(id.to_owned(), e.clone());
                    }
                    None => {
                        self.project.lock.parts.remove(id);
                    }
                }
                for a in plan.assignments.iter().filter(|a| a.part == id) {
                    unmatched.push(format!(
                        "line {}: part {id} cannot be taken into the project: {why}",
                        a.line
                    ));
                }
                taken.assignments.retain(|a| a.part != id);
            }
        }
        // Applied to a copy first: an undo step only for a change that is made, and pins only
        // for the parts the result uses.
        let mut staged = self.project.clone();
        let changed = wc_import::wago::apply(&mut staged, &taken);
        let used = staged.used_parts();
        for id in plan.parts() {
            if !used.contains(id) && !lock.parts.contains_key(id) {
                self.project.lock.parts.remove(id);
            }
        }
        if changed > 0 {
            self.checkpoint_with_lock(lock);
            wc_import::wago::apply(&mut self.project, &taken);
        } else {
            // Nothing applies: no pin either, so the lock stays as saved and undoable.
            self.project.lock = lock;
        }
        let mut msg = format!(
            "WAGO configurator: {changed} device(s) or jumper(s) changed, {} row(s) already up to date, {} not matched.",
            plan.unchanged,
            unmatched.len()
        );
        if !unmatched.is_empty() {
            msg.push_str("\n\nNot matched:");
            for u in &unmatched {
                msg.push_str("\n- ");
                msg.push_str(u);
            }
        }
        for w in warnings {
            msg.push_str("\n\nWarning: ");
            msg.push_str(&w);
        }
        Ok(msg)
    }

    /// Make sure a part is in the project (taken from the library when needed) and loaded.
    fn ensure_part(&mut self, id: &str) -> Result<(), String> {
        let pin = self.project.lock.parts.get(id).cloned();
        if pin.is_some() && self.parts.contains_key(id) {
            return Ok(());
        }
        let vendored = store::vendor_path(&self.dir, ItemKind::Part, id).map_err(|e| e.to_string())?;
        let lib = || wc_library::Library::open(&self.dir, &self.project).map_err(|e| e.to_string());
        let exists = vendored
            .try_exists()
            .map_err(|e| format!("part {id}: {} cannot be checked: {e}", vendored.display()))?;
        match (&pin, exists) {
            // No copy of a pinned part: put back from the library only in the version it pins,
            // the very part checked, and the pin kept as it is.
            (Some(pin), false) => {
                // The source the pin names first, then every other: any may still have the
                // pinned version when the named source has moved on.
                let fits = |p: &Part| p.hash() == pin.hash && p.id == id;
                let p = lib()?
                    .find_version(ItemKind::Part, id, &pin.source, fits)
                    .map_err(|why| {
                        format!(
                            "part {id}: its copy in the project is missing and no library has the version the project pins ({why}); put the copy back or update the part from the library"
                        )
                    })?;
                store::write_vendored_part(&self.dir, &p).map_err(|e| e.to_string())?;
            }
            // No copy and no pin: a new part, taken from the library and pinned.
            (None, false) => {
                let lib = lib()?;
                wc_library::take(&self.dir, &mut self.project, &lib, ItemKind::Part, id).map_err(|e| e.to_string())?;
            }
            // A pinned copy: used only when it is the version the pin names.
            (Some(pin), true) => {
                let copy = store::read_vendored_part(&self.dir, id).map_err(|e| e.to_string())?;
                if copy.hash() != pin.hash || copy.id != id {
                    return Err(format!(
                        "part {id}: its copy in the project is not the version the project pins; put the pinned copy back or update the part from the library"
                    ));
                }
            }
            // A copy without its pin (an undone change took the pin back): pinned again as it
            // is, never replaced by a newer library version, to the library that has the part.
            (None, true) => {
                let copy = store::read_vendored_part(&self.dir, id).map_err(|e| e.to_string())?;
                let (found, source) = lib()?.part(id).map_err(|e| match e {
                    wc_library::Error::NotFound { .. } => {
                        format!("part {id} has a copy in the project but no pin, and no library has it to pin it to")
                    }
                    e => format!("part {id} has a copy in the project but no pin, and its library file cannot be read to pin it: {e}"),
                })?;
                if found.id != id {
                    return Err(format!(
                        "part {id} has a copy in the project but no pin, and the library file for it ({source}) declares id {:?}",
                        found.id
                    ));
                }
                self.project.lock.parts.insert(
                    id.to_owned(),
                    wc_core::LockEntry {
                        source,
                        hash: copy.hash(),
                    },
                );
            }
        }
        let p = store::read_vendored_part(&self.dir, id).map_err(|e| e.to_string())?;
        self.parts.insert(id.to_owned(), p);
        Ok(())
    }

    /// Replace a device's accessories (its parts after the main part). New parts are taken from
    /// the library into the project; a quantity of 0 is taken as 1. One undo step.
    pub fn set_device_accessories(&mut self, tag: &DeviceTag, accessories: Vec<DevicePart>) -> Result<(), String> {
        let accessories: Vec<DevicePart> = accessories
            .into_iter()
            .map(|a| DevicePart { qty: a.qty.max(1), ..a })
            .collect();
        if self.project.devices.get(tag).map_or(&[][..], |d| &d.accessories) == accessories.as_slice() {
            return Ok(());
        }
        let lock = self.project.lock.clone();
        for a in &accessories {
            if let Err(e) = self.ensure_part(&a.part) {
                // Parts taken before this one are not pinned without a change that uses them.
                self.project.lock = lock;
                return Err(e);
            }
        }
        self.checkpoint_with_lock(lock);
        self.project.devices.entry(tag.clone()).or_default().accessories = accessories;
        Ok(())
    }

    /// Set (or with an empty value, remove) one of a device's own properties.
    pub fn set_device_property(&mut self, tag: &DeviceTag, key: &str, value: &str) {
        let (key, value) = (key.trim(), value.trim());
        if key.is_empty() {
            return;
        }
        let current = self.project.devices.get(tag).and_then(|d| d.properties.get(key));
        if current.map(String::as_str) == Some(value) || (current.is_none() && value.is_empty()) {
            return;
        }
        self.checkpoint();
        let d = self.project.devices.entry(tag.clone()).or_default();
        if value.is_empty() {
            d.properties.remove(key);
        } else {
            d.properties.insert(key.to_owned(), value.to_owned());
        }
    }

    /// Run `f` and then take back everything it changed in the project and its undo history:
    /// for showing a form on a page someone else holds, where a field that still commits must
    /// leave no trace (no change, no undo step, redo kept, not marked as changed).
    pub(crate) fn without_changes<R>(&mut self, f: impl FnOnce(&mut Self) -> R) -> R {
        let project = self.project.clone();
        let undo = std::mem::take(&mut self.undo);
        let redo = std::mem::take(&mut self.redo);
        let (undo_dropped, history, form_step, dirty) = (self.undo_dropped, self.history, self.form_step, self.dirty);
        let terminals_before = self.terminals_before.clone();
        let r = f(self);
        self.project = project;
        self.undo = undo;
        self.redo = redo;
        (self.undo_dropped, self.history, self.form_step, self.dirty) = (undo_dropped, history, form_step, dirty);
        self.terminals_before = terminals_before;
        r
    }

    /// Take a snapshot before a change, so it can be undone.
    /// [`Open::checkpoint`] for a change that pinned library items before it: the undo step
    /// keeps the lock as it was (`lock`), so Undo takes those pins back and Redo brings them.
    fn checkpoint_with_lock(&mut self, lock: wc_core::Lockfile) {
        let now = std::mem::replace(&mut self.project.lock, lock);
        self.checkpoint();
        self.project.lock = now;
    }

    pub fn checkpoint(&mut self) {
        // The change before this one is done: its jumpers follow their terminals first, so the
        // snapshot and this change start from that.
        self.follow_terminals();
        self.terminals_before = Some(self.terminal_numbers());
        self.undo.push(self.project.clone());
        if self.undo.len() > UNDO_STEPS {
            self.undo.remove(0);
            self.undo_dropped += 1;
        }
        self.redo.clear();
        self.dirty = true;
        self.history += 1;
    }

    /// Which terminal each placed terminal symbol is, as the terminal plan numbers them.
    pub fn terminal_numbers(&self) -> wc_core::project::TerminalNumbers {
        wc_core::project::terminal_numbers(&self.project, |id| self.symbols.get(id).map(|s| s.role))
    }

    /// Keep jumpers on the terminals they joined, after the change since the last checkpoint
    /// (a terminal renamed, moved along its strip or deleted). Returns whether a jumper changed.
    pub fn follow_terminals(&mut self) -> bool {
        let Some(before) = self.terminals_before.take() else {
            return false;
        };
        let symbols = &self.symbols;
        let changed =
            wc_core::project::follow_terminals_in(&mut self.project, &before, |id| symbols.get(id).map(|s| s.role));
        // Keep tracking: the next change before a checkpoint (the next frame of one drag) is
        // measured from here. Without jumpers there is nothing to follow until the next one.
        if !self.project.meta.jumpers.is_empty() {
            self.terminals_before = Some(self.terminal_numbers());
        }
        changed
    }

    /// Forget the change in progress without following it: the project was replaced as a whole
    /// (undo, redo, a command's result), and its jumpers already belong to its drawing.
    pub fn forget_terminal_change(&mut self) {
        self.terminals_before = None;
    }

    /// A mark in the undo history, for [`Open::squash_undo`].
    pub fn undo_mark(&self) -> usize {
        self.undo_dropped + self.undo.len()
    }

    /// Make every change since `mark` one undo step, so one Undo takes all of it back
    /// (the assistant's changes in one turn, for example).
    pub fn squash_undo(&mut self, mark: usize) {
        let Some(start) = mark.checked_sub(self.undo_dropped) else {
            return;
        };
        if self.undo.len() > start + 1 {
            self.undo.truncate(start + 1);
            self.history += 1;
        }
    }

    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }

    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }

    pub fn undo(&mut self) -> bool {
        // The change being undone is followed first, so redo brings it back whole.
        self.follow_terminals();
        match self.undo.pop() {
            Some(prev) => {
                self.forget_terminal_change();
                self.redo.push(std::mem::replace(&mut self.project, prev));
                if let Err(e) = self.sync_symbols() {
                    // The symbol files could not follow: keep the project as it was.
                    if let Some(cur) = self.redo.pop() {
                        self.undo.push(std::mem::replace(&mut self.project, cur));
                    }
                    self.undo_error = Some(e);
                    return false;
                }
                let sources = self.redo.last().map(|p| &p.meta.libraries) != Some(&self.project.meta.libraries);
                self.follow_step(sources);
                self.dirty = true;
                self.history += 1;
                true
            }
            None => false,
        }
    }

    pub fn redo(&mut self) -> bool {
        self.follow_terminals();
        match self.redo.pop() {
            Some(next) => {
                self.forget_terminal_change();
                self.undo.push(std::mem::replace(&mut self.project, next));
                if let Err(e) = self.sync_symbols() {
                    if let Some(cur) = self.undo.pop() {
                        self.redo.push(std::mem::replace(&mut self.project, cur));
                    }
                    self.undo_error = Some(e);
                    return false;
                }
                let sources = self.undo.last().map(|p| &p.meta.libraries) != Some(&self.project.meta.libraries);
                self.follow_step(sources);
                self.dirty = true;
                self.history += 1;
                true
            }
            None => false,
        }
    }

    /// After an undo or redo: rebuild the library list when the step added or removed a library
    /// source (`sources_changed`), so symbols of a source that is gone are no longer offered and
    /// those of one that came back are; then load every part a device uses that is not loaded,
    /// since a redo can bring back an assignment whose part an earlier rebuild dropped.
    fn follow_step(&mut self, sources_changed: bool) {
        if sources_changed {
            self.refresh_library();
        }
        let missing: Vec<String> = self
            .project
            .used_parts()
            .into_iter()
            .filter(|id| !self.parts.contains_key(id))
            .collect();
        let mut failed = Vec::new();
        for id in missing {
            match store::read_vendored_part(&self.dir, &id) {
                Ok(p) => {
                    self.parts.insert(id, p);
                }
                Err(e) => failed.push(format!("{id} ({e})")),
            }
        }
        self.step_warning =
            (!failed.is_empty()).then(|| format!("these parts could not be loaded: {}", failed.join("; ")));
    }

    /// Load the vendored symbols again after the library changed them, keeping the versions
    /// they replace so Undo can bring those back.
    pub fn reload_symbols(&mut self) {
        for sym in self.symbols.values() {
            self.symbol_versions.insert(sym.hash(), sym.clone());
        }
        let (mut syms, _) = store::vendored_symbols(&self.dir, &self.project);
        // Locked symbols no page uses yet are kept too, so the version the lock names is
        // known for Redo and for the next placement.
        for id in self.project.lock.symbols.keys() {
            if !syms.contains_key(id)
                && let Ok(sym) = store::read_vendored_symbol(&self.dir, id)
            {
                syms.insert(id.clone(), sym);
            }
        }
        // A file that could not be put back still holds another version: the loaded one stays
        // while the lock still names it, so the next save writes it. A lock that now names
        // the file's version (the symbol was taken again) makes the file right.
        for id in self.stale_symbol_files.clone() {
            let wanted = self.project.lock.symbols.get(&id).map(|e| e.hash.clone());
            match self.symbols.get(&id) {
                Some(loaded) if wanted.as_ref().is_none_or(|h| *h == loaded.hash()) => {
                    syms.insert(id.clone(), loaded.clone());
                }
                _ => {
                    self.stale_symbol_files.remove(&id);
                }
            }
        }
        self.symbols = syms;
    }

    /// After Undo or Redo: put back the vendored symbol versions the restored lock names, both
    /// in memory and in the project folder, so the pages draw what the lock says. When a file
    /// cannot be written, the files already written are put back and nothing changes.
    fn sync_symbols(&mut self) -> Result<(), String> {
        let mut wanted: Vec<(String, Symbol)> = Vec::new();
        for (id, entry) in &self.project.lock.symbols {
            if let Some(loaded) = self.symbols.get(id).filter(|s| s.hash() == entry.hash) {
                // Loaded already; its file is written again only when it could not be put back.
                if self.stale_symbol_files.contains(id) {
                    wanted.push((id.clone(), loaded.clone()));
                }
                continue;
            }
            // The version the lock names: kept from earlier, or still in the project folder.
            if let Some(sym) = self.symbol_versions.get(&entry.hash) {
                wanted.push((id.clone(), sym.clone()));
                continue;
            }
            match store::read_vendored_symbol(&self.dir, id) {
                Ok(sym) if sym.hash() == entry.hash => wanted.push((id.clone(), sym)),
                Ok(_) => {
                    return Err(format!(
                        "the version of symbol {id} this step names is not known any more"
                    ));
                }
                Err(e) => {
                    return Err(format!(
                        "the version of symbol {id} this step names is not known any more, and its file cannot be read: {e}"
                    ));
                }
            }
        }
        for (n, (id, want)) in wanted.iter().enumerate() {
            if let Err(e) = self.write_symbol_file(want) {
                // Put back the files already written; say which could not be, and remember
                // them so the next Undo or Redo writes them again.
                let mut unrestored = Vec::new();
                for (done, _) in wanted.iter().take(n) {
                    let Some(old) = self.symbols.get(done).cloned() else {
                        continue;
                    };
                    if let Err(e) = self.write_symbol_file(&old) {
                        unrestored.push(format!("{done} ({e})"));
                        self.stale_symbol_files.insert(done.clone());
                    }
                }
                let mut msg = format!("symbol {id} could not be put back: {e}");
                if !unrestored.is_empty() {
                    msg.push_str(&format!(
                        "; these symbol files now hold the other version and could not be restored: {}. The next Undo or Redo writes them again.",
                        unrestored.join(", ")
                    ));
                }
                return Err(msg);
            }
        }
        for (id, _) in &wanted {
            self.stale_symbol_files.remove(id);
        }
        for (id, want) in wanted {
            if let Some(old) = self.symbols.insert(id, want) {
                self.symbol_versions.insert(old.hash(), old);
            }
        }
        // A symbol the restored step neither locks nor uses is let go, kept as a version for
        // Redo: still loaded, it would let the next placement skip pinning it in the lock.
        let used = self.project.used_symbols();
        let gone: Vec<String> = self
            .symbols
            .keys()
            .filter(|id| !self.project.lock.symbols.contains_key(*id) && !used.contains(*id))
            .cloned()
            .collect();
        for id in gone {
            if let Some(old) = self.symbols.remove(&id) {
                self.symbol_versions.insert(old.hash(), old);
            }
        }
        Ok(())
    }

    pub(crate) fn write_symbol_file(&mut self, sym: &Symbol) -> Result<(), String> {
        #[cfg(test)]
        if self.fail_symbol_writes.pop_front() == Some(true) {
            return Err("test write failure".into());
        }
        store::write_vendored_symbol(&self.dir, sym)
            .map(|_| ())
            .map_err(|e| e.to_string())
    }

    /// Remember that a symbol's vendored file may hold another version than the loaded one;
    /// the next save, Undo or Redo writes the loaded version again.
    pub(crate) fn mark_symbol_file_stale(&mut self, id: &str) {
        self.stale_symbol_files.insert(id.to_owned());
    }

    /// Write the loaded version of every symbol whose file may hold another one. A save does
    /// not go ahead while one of them cannot be written, since the saved lock would not match.
    fn rewrite_stale_symbol_files(&mut self) -> Result<(), String> {
        for id in self.stale_symbol_files.clone() {
            let Some(sym) = self.symbols.get(&id).cloned() else {
                self.stale_symbol_files.remove(&id);
                continue;
            };
            self.write_symbol_file(&sym).map_err(|e| {
                format!("not saved: the file of symbol {id} still holds another version and cannot be written: {e}")
            })?;
            self.stale_symbol_files.remove(&id);
        }
        Ok(())
    }

    /// Keep the loaded symbol versions, so a step that replaces them can be undone.
    pub fn archive_symbols(&mut self) {
        for sym in self.symbols.values() {
            self.symbol_versions.insert(sym.hash(), sym.clone());
        }
    }

    /// Take the newer library versions of the project's items (only `only`, when given), as
    /// one undo step. An update that would leave wires on pins a new symbol version no longer
    /// has is taken back, and the wires are named.
    pub fn update_library(
        &mut self,
        lib: &wc_library::Library,
        only: Option<&[String]>,
    ) -> Result<Vec<(ItemKind, String)>, String> {
        let all = wc_library::status(&self.dir, &self.project, lib);
        // An item named on purpose whose library file is wrong stops the update.
        if let Some(bad) = wc_library::wrong_named(&all, only) {
            return Err(bad.to_string());
        }
        // Exactly these versions are taken, as seen here: an item whose library file changes
        // after this (from a wrong id to a new version, or to yet another version) is not
        // written without a backup; it waits for the next update.
        let todo = wc_library::changed(&all, only);
        // Checked before anything is written: a new version that drops a wired pin stops the
        // whole update.
        // The pins are checked on the very version the update will take: one that changed in the
        // library since it was looked at is not taken at all.
        let mut dangling: Vec<String> = Vec::new();
        for (kind, id, hash) in &todo {
            let Some((s, _)) = (*kind == ItemKind::Symbol).then(|| lib.symbol(id).ok()).flatten() else {
                continue;
            };
            if s.hash() != *hash {
                return Err(format!(
                    "nothing was updated: the symbol {id} changed in the library while it was being updated; update again"
                ));
            }
            dangling.extend(dangling_pins(&self.project, id, &s));
        }
        if !dangling.is_empty() {
            dangling.sort();
            return Err(format!(
                "nothing was updated: wires end on pins the new versions no longer have ({}). Move those wires first.",
                dangling.join(", ")
            ));
        }
        // Every file the update may write, and the lock, are kept here: a failure puts them
        // back directly, whatever undo can or cannot restore.
        let files: Vec<(PathBuf, Option<Vec<u8>>)> = todo
            .iter()
            .filter_map(|(kind, id, _)| store::vendor_path(&self.dir, *kind, id).ok())
            .map(|path| {
                #[cfg(test)]
                if self.unreadable.as_ref() == Some(&path) {
                    return Err(format!("{} cannot be read: test read failure", path.display()));
                }
                Ok((path.clone(), snapshot_file(&path)?))
            })
            .collect::<Result<_, String>>()
            .map_err(|e| format!("nothing was updated: {e}"))?;
        let lock = self.project.lock.clone();
        let history = HistoryState {
            redo: self.redo.clone(),
            evicted: (self.undo.len() >= UNDO_STEPS)
                .then(|| self.undo.first().cloned())
                .flatten(),
            dirty: self.dirty,
            history: self.history,
        };
        #[cfg(test)]
        if let Some(path) = self.block_after_snapshot.take() {
            let _ = std::fs::remove_file(&path);
            let _ = std::fs::create_dir(&path);
        }
        #[cfg(test)]
        if let Some((path, text)) = self.rewrite_after_snapshot.take() {
            std::fs::write(&path, text).map_err(|e| format!("test: {} cannot be rewritten: {e}", path.display()))?;
        }
        self.archive_symbols();
        self.checkpoint();
        let done = match wc_library::update_items(&self.dir, &mut self.project, lib, &todo) {
            Ok(done) => done,
            Err(e) => {
                // `update_items` put back what it wrote; only the undo step goes.
                let unrestored = self.roll_back_update(&[], lock, history);
                return Err(if unrestored.is_empty() {
                    e.to_string()
                } else {
                    format!(
                        "{e}; what was already updated could not be taken back: {}",
                        unrestored.join("; ")
                    )
                });
            }
        };
        self.reload_symbols();
        // The loaded parts follow their new files too, so a copy compared with the lock later
        // (after a refused session update puts the old pin back) is the version on disk.
        for (kind, id) in &done {
            if *kind == ItemKind::Part {
                match store::read_vendored_part(&self.dir, id) {
                    Ok(p) => {
                        self.parts.insert(id.clone(), p);
                    }
                    Err(_) => {
                        self.parts.remove(id);
                    }
                }
            }
        }
        let mut dangling: Vec<String> = done
            .iter()
            .filter(|(kind, _)| *kind == ItemKind::Symbol)
            .filter_map(|(_, id)| Some(dangling_pins(&self.project, id, self.symbols.get(id)?)))
            .flatten()
            .collect();
        if dangling.is_empty() {
            return Ok(done);
        }
        dangling.sort();
        let wires = dangling.join(", ");
        let unrestored = self.roll_back_update(&files, lock, history);
        if !unrestored.is_empty() {
            return Err(format!(
                "wires end on pins the new versions no longer have ({wires}), and the update could not be taken back: {}",
                unrestored.join("; ")
            ));
        }
        Err(format!(
            "nothing was updated: wires end on pins the new versions no longer have ({wires}). Move those wires first."
        ))
    }

    /// Take back a library update that failed: the files it wrote and the lock as they were
    /// (`files`, `lock`), its undo step dropped, and the history as it was before it (`was`: the
    /// Redo list, the oldest Undo step its checkpoint pushed out, whether there were unsaved
    /// changes). Returns what could not be put back.
    fn roll_back_update(
        &mut self,
        files: &[(PathBuf, Option<Vec<u8>>)],
        lock: wc_core::Lockfile,
        was: HistoryState,
    ) -> Vec<String> {
        let mut unrestored: Vec<String> = files
            .iter()
            .filter_map(|(path, was)| {
                restore_file(path, was.as_deref())
                    .err()
                    .map(|err| format!("{} ({err})", path.display()))
            })
            .collect();
        self.reload_symbols();
        if !self.undo()
            && let Some(why) = self.take_undo_error()
        {
            unrestored.push(why);
        }
        // Whatever undo managed, the lock is the one the files above match.
        self.project.lock = lock;
        self.redo = was.redo;
        if let Some(oldest) = was.evicted {
            self.undo.insert(0, oldest);
            self.undo_dropped = self.undo_dropped.saturating_sub(1);
        }
        self.dirty = was.dirty;
        self.history = was.history;
        self.reload_symbols();
        unrestored
    }

    /// Why the last Undo or Redo failed, once.
    pub fn take_undo_error(&mut self) -> Option<String> {
        self.undo_error.take()
    }

    /// Parts the last Undo or Redo could not load, though the step was done; once.
    pub fn take_step_warning(&mut self) -> Option<String> {
        self.step_warning.take()
    }

    /// After the symbol editor saved symbol `id` into the library: take that symbol (only the
    /// symbol, never a part with the same id) into the project, as one undo step.
    pub fn take_saved_symbol(&mut self, lib: &wc_library::Library, id: &str) -> Result<(), String> {
        // Taken into a copy first: a failure leaves the project and its history as they were
        // (take writes the file only once the library symbol is read, and nothing after).
        self.archive_symbols();
        let mut next = self.project.clone();
        let entry = wc_library::take(&self.dir, &mut next, lib, ItemKind::Symbol, id).map_err(|e| e.to_string())?;
        self.checkpoint();
        self.project.lock.symbols.insert(id.to_owned(), entry);
        self.reload_symbols();
        Ok(())
    }

    pub fn save(&mut self) -> Result<usize, String> {
        self.follow_terminals();
        self.rewrite_stale_symbol_files()?;
        let n = store::save(&self.dir, &self.project).map_err(|e| e.to_string())?.len();
        self.dirty = false;
        Ok(n)
    }

    /// Make sure a library symbol is vendored and pinned before it is placed. A symbol the
    /// project already pins (one no page uses is not loaded at open) is placed in the version
    /// it pins: its copy in the project, or that version put back from the library. Only a
    /// symbol with no pin is taken from the library as it is now.
    pub fn ensure_symbol(&mut self, id: &str) -> Result<(), String> {
        if let Some(loaded) = self.symbols.get(id) {
            // Loaded at open from its file, which may hold another version than the pin names.
            return match self.project.lock.symbols.get(id) {
                Some(pin) if loaded.hash() != pin.hash => Err(format!(
                    "symbol {id}: its copy in the project is not the version the project pins; put the pinned copy back or update the symbol from the library"
                )),
                _ => Ok(()),
            };
        }
        let lib = || wc_library::Library::open(&self.dir, &self.project).map_err(|e| e.to_string());
        let s = match self.project.lock.symbols.get(id).cloned() {
            Some(pin) => {
                let vendored = store::vendor_path(&self.dir, ItemKind::Symbol, id).map_err(|e| e.to_string())?;
                let exists = vendored
                    .try_exists()
                    .map_err(|e| format!("symbol {id}: {} cannot be checked: {e}", vendored.display()))?;
                let fits = |s: &Symbol| s.hash() == pin.hash && s.id == id;
                if exists {
                    let copy = store::read_vendored_symbol(&self.dir, id).map_err(|e| e.to_string())?;
                    if !fits(&copy) {
                        return Err(format!(
                            "symbol {id}: its copy in the project is not the version the project pins; put the pinned copy back or update the symbol from the library"
                        ));
                    }
                    copy
                } else {
                    // The source the pin names first, then every other: any may still have the
                    // pinned version.
                    let s = lib()?
                        .find_version(ItemKind::Symbol, id, &pin.source, fits)
                        .map_err(|why| {
                            format!(
                                "symbol {id}: its copy in the project is missing and no library has the version the project pins ({why}); put the copy back or update the symbol from the library"
                            )
                        })?;
                    store::write_vendored_symbol(&self.dir, &s).map_err(|e| e.to_string())?;
                    s
                }
            }
            None => {
                let lib = lib()?;
                wc_library::take(&self.dir, &mut self.project, &lib, ItemKind::Symbol, id)
                    .map_err(|e| e.to_string())?;
                store::read_vendored_symbol(&self.dir, id).map_err(|e| e.to_string())?
            }
        };
        self.symbols.insert(id.to_owned(), s);
        Ok(())
    }

    /// Suggest the next free tag for a letter code, e.g. `=PUMP+CAB1-K3`. Function and location
    /// are taken from the tags already on the page (the most common pair), so a new device lands
    /// in the same plant section and cabinet as its neighbours.
    pub fn next_tag(&self, page: usize, letter: &str) -> Option<DeviceTag> {
        if letter.is_empty() {
            return None;
        }
        let mut counts: BTreeMap<(Vec<String>, Vec<String>), usize> = BTreeMap::new();
        for t in self
            .project
            .pages
            .get(page)
            .into_iter()
            .flat_map(|p| p.symbols.iter())
            .filter_map(|s| s.tag.as_ref())
        {
            *counts.entry((t.function.clone(), t.location.clone())).or_default() += 1;
        }
        let (function, location) = counts
            .into_iter()
            .max_by_key(|(_, n)| *n)
            .map(|(k, _)| k)
            .unwrap_or_default();
        let used: std::collections::BTreeSet<String> =
            self.project.tag_index().keys().map(|t| t.product.join(".")).collect();
        let product = (1..10_000)
            .map(|n| format!("{letter}{n}"))
            .find(|p| !used.contains(p))?;
        let mut text = String::new();
        if !function.is_empty() {
            text.push_str(&format!("={}", function.join(".")));
        }
        if !location.is_empty() {
            text.push_str(&format!("+{}", location.join(".")));
        }
        text.push_str(&format!("-{product}"));
        DeviceTag::parse(&text).ok()
    }

    pub fn place(&mut self, page: usize, symbol: &str, at: Point) -> Result<String, String> {
        self.ensure_symbol(symbol)?;
        let letter = self
            .symbols
            .get(symbol)
            .map(|s| s.letter_code.clone())
            .unwrap_or_default();
        let tag = self.next_tag(page, &letter);
        self.checkpoint();
        if let Some(t) = &tag {
            self.project.devices.entry(t.clone()).or_default();
        }
        let pg = self.project.pages.get_mut(page).ok_or("no page")?;
        let id = pg.next_id("s");
        pg.symbols.push(PlacedSymbol {
            id: id.clone(),
            symbol: symbol.to_owned(),
            at,
            rotation: Rotation::R0,
            mirror: false,
            tag,
        });
        Ok(id)
    }

    pub fn add_wire(&mut self, page: usize, from: Endpoint, to: Endpoint, via: Vec<Point>) {
        self.checkpoint();
        if let Some(pg) = self.project.pages.get_mut(page) {
            let id = pg.next_id("w");
            pg.wires.push(Wire {
                id,
                from,
                to,
                via,
                number: None,
                potential: None,
                cable: None,
                route: Default::default(),
                spec: Default::default(),
            });
        }
    }

    /// Draw a wire between each pair of pins that line up or touch, for the symbols in `items`
    /// (just placed or moved). Part of the caller's undo step: no checkpoint is taken here.
    /// Returns how many wires were added.
    pub fn auto_wire(&mut self, page: usize, items: &std::collections::BTreeSet<String>) -> usize {
        let Some(pg) = self.project.pages.get(page) else {
            return 0;
        };
        let pairs = wc_core::autowire::suggest(pg, |id| self.symbols.get(id), items);
        let Some(pg) = self.project.pages.get_mut(page) else {
            return 0;
        };
        for (from, to) in &pairs {
            let id = pg.next_id("w");
            pg.wires.push(Wire {
                id,
                from: from.clone(),
                to: to.clone(),
                via: Vec::new(),
                number: None,
                potential: None,
                cable: None,
                route: Default::default(),
                spec: Default::default(),
            });
        }
        if !pairs.is_empty() {
            self.dirty = true;
        }
        pairs.len()
    }

    pub fn add_page(&mut self) -> usize {
        self.checkpoint();
        let id = self.free_page_id();
        let number = self.next_page_number();
        self.project.pages.push(Page::new(id.clone(), number, "New page"));
        self.project.sort_pages();
        self.index_of(&id)
    }

    fn free_page_id(&self) -> String {
        let mut n = 1;
        while self.project.pages.iter().any(|p| p.id == format!("p{n:04}")) {
            n += 1;
        }
        format!("p{n:04}")
    }

    fn index_of(&self, id: &str) -> usize {
        self.project.pages.iter().position(|p| p.id == id).unwrap_or(0)
    }

    /// Change a page's number, title and size, and with `fields` its own title-block fields
    /// (key to value; a blank value clears the field so the project's shows again, a key left
    /// out is kept). A number another page already uses is refused. All of it is one undo step.
    pub fn set_page_props(
        &mut self,
        page: usize,
        number: &str,
        title: &str,
        size: PageSize,
        overview: bool,
        fields: Option<&BTreeMap<String, String>>,
    ) -> Result<usize, String> {
        let number = number.trim();
        if number.is_empty() {
            return Err("The page number cannot be empty.".into());
        }
        let Some(current) = self.project.pages.get(page) else {
            return Err("No such page.".into());
        };
        let id = current.id.clone();
        if self.project.pages.iter().any(|p| p.number == number && p.id != id) {
            return Err(format!("Page {number} already exists."));
        }
        let mut own = current.clone();
        for (k, v) in fields.into_iter().flatten() {
            own.set_title_field(k, v);
        }
        if current.number == number
            && current.title == title
            && current.size == size
            && current.overview == overview
            && current.title_block == own.title_block
        {
            return Ok(page);
        }
        self.checkpoint();
        if let Some(p) = self.project.pages.get_mut(page) {
            p.number = number.to_owned();
            p.title = title.trim().to_owned();
            p.size = size;
            p.overview = overview;
            p.title_block = own.title_block;
        }
        self.project.sort_pages();
        Ok(self.index_of(&id))
    }

    /// Why `page` cannot be changed here: in a shared session someone else has it open.
    pub fn held_reason(&self, page: usize) -> Option<String> {
        let id = &self.project.pages.get(page)?.id;
        self.held_by_others
            .get(id)
            .map(|name| format!("{name} has this page open; it is read-only here until they leave it."))
    }

    /// The pages someone else holds, as they are now: what [`Open::held_change`] compares with.
    pub fn held_pages(&self) -> Vec<Page> {
        self.project
            .pages
            .iter()
            .filter(|p| self.held_by_others.contains_key(&p.id))
            .cloned()
            .collect()
    }

    /// Why a change is refused: it changed or removed one of the pages someone else held when
    /// [`Open::held_pages`] gave `before`.
    pub fn held_change(&self, before: &[Page]) -> Option<String> {
        before.iter().find_map(|old| {
            let who = self.held_by_others.get(&old.id).map_or("Someone", String::as_str);
            match self.project.pages.iter().position(|p| p.id == old.id) {
                None => Some(format!(
                    "it would remove page {}, which {who} has open; it is read-only here until they leave it.",
                    old.number
                )),
                Some(i) if self.project.pages[i] != *old => Some(format!(
                    "it would change page {}, which {who} has open; it is read-only here until they leave it.",
                    old.number
                )),
                Some(_) => None,
            }
        })
    }

    /// Start a change that may be refused as a whole: the project and the symbols and parts
    /// loaded now are kept, and the undo history is set aside, so the change's own undo steps
    /// collect apart from it. [`Open::keep`] or [`Open::back_to`] ends it. Undo and redo
    /// must not run in between.
    /// The steps set aside still count in [`Open::undo_mark`], so a mark taken in between
    /// (a form opened by the change) stays valid after [`Open::keep`].
    pub(crate) fn mark(&mut self) -> Mark {
        let undo = std::mem::take(&mut self.undo);
        let undo_dropped = self.undo_dropped;
        self.undo_dropped += undo.len();
        Mark {
            project: self.project.clone(),
            symbols: self.symbols.keys().cloned().collect(),
            parts: self.parts.keys().cloned().collect(),
            undo,
            redo: std::mem::take(&mut self.redo),
            undo_dropped,
            history: self.history,
            form_step: self.form_step,
            dirty: self.dirty,
            terminals_before: self.terminals_before.clone(),
            files: vendor_names(&self.dir),
        }
    }

    /// The change since `mark` stays: its undo steps go after the earlier ones (the oldest
    /// dropped beyond the usual limit), and redo is kept only when it made no undo step.
    pub(crate) fn keep(&mut self, mark: Mark) {
        let mut undo = mark.undo;
        // The set-aside steps counted as dropped while the change ran; they are back now.
        self.undo_dropped -= undo.len();
        undo.append(&mut self.undo);
        let over = undo.len().saturating_sub(UNDO_STEPS);
        undo.drain(..over);
        self.undo_dropped += over;
        self.undo = undo;
        if self.history == mark.history {
            self.redo = mark.redo;
        }
    }

    /// Take back everything changed since `mark`: the project, the symbols and parts it
    /// loaded, its undo steps, and the change marks; the undo and redo history are as before.
    /// A symbol or part file the change added (there was none, and the project as it was does
    /// not pin it) is removed; files that were there are never touched, nor rewritten. One that
    /// cannot be removed is reported, and is harmless: an unpinned part copy is pinned again
    /// when it is next used.
    pub(crate) fn back_to(&mut self, mark: Mark) -> Result<(), String> {
        self.project = mark.project;
        self.symbols.retain(|id, _| mark.symbols.contains(id));
        self.parts.retain(|id, _| mark.parts.contains(id));
        self.undo = mark.undo;
        self.redo = mark.redo;
        self.undo_dropped = mark.undo_dropped;
        (self.history, self.form_step, self.dirty) = (mark.history, mark.form_step, mark.dirty);
        self.terminals_before = mark.terminals_before;
        let (before, now) = match (mark.files, vendor_names(&self.dir)) {
            (Ok(before), Ok(now)) => (before, now),
            (Err(e), _) | (_, Err(e)) => {
                return Err(format!("Library files it may have added were not checked: {e}"));
            }
        };
        let lock = &self.project.lock;
        let mut failed = Vec::new();
        for (kind, id) in now.difference(&before) {
            let pinned = match kind {
                ItemKind::Symbol => lock.symbols.contains_key(id),
                ItemKind::Part => lock.parts.contains_key(id),
            };
            if pinned {
                continue;
            }
            if let Ok(path) = store::vendor_path(&self.dir, *kind, id)
                && let Err(e) = std::fs::remove_file(&path)
            {
                failed.push(format!("{}: {e}", path.display()));
            }
        }
        if failed.is_empty() {
            Ok(())
        } else {
            Err(format!(
                "Library files it added could not be removed: {}",
                failed.join("; ")
            ))
        }
    }

    /// Delete a page. The last page of a project cannot be deleted. Returns the page to show next.
    pub fn delete_page(&mut self, page: usize) -> Result<usize, String> {
        if self.project.pages.len() <= 1 {
            return Err("A project needs at least one page.".into());
        }
        if page >= self.project.pages.len() {
            return Err("No such page.".into());
        }
        if let Some(why) = self.held_reason(page) {
            return Err(why);
        }
        self.checkpoint();
        self.project.pages.remove(page);
        Ok(page.min(self.project.pages.len() - 1))
    }

    /// Move a page one place up or down by swapping its number with its neighbour's.
    /// Returns the page's new index, or `None` when it is already at that end.
    pub fn move_page(&mut self, page: usize, up: bool) -> Option<usize> {
        let other = if up { page.checked_sub(1)? } else { page + 1 };
        if other >= self.project.pages.len() || page >= self.project.pages.len() {
            return None;
        }
        // Both pages get a new number: refused when someone else has either open.
        if self.held_reason(page).is_some() || self.held_reason(other).is_some() {
            return None;
        }
        self.checkpoint();
        let id = self.project.pages[page].id.clone();
        let a = self.project.pages[page].number.clone();
        let b = std::mem::replace(&mut self.project.pages[other].number, a);
        self.project.pages[page].number = b;
        self.project.sort_pages();
        Some(self.index_of(&id))
    }

    /// Copy a page as a new last page. Devices on the copy get the next free tags (one new device
    /// per original device, keeping description and part) and wire numbers are cleared.
    /// Macros of the project (`macros/`) and of its library sources, project first.
    pub fn macros(&self) -> Vec<MacroEntry> {
        let mut found: Vec<(String, String, PathBuf)> = wc_core::macros::list_macros(&self.dir)
            .into_iter()
            .map(|(id, path)| ("project".to_owned(), id, path))
            .collect();
        if let Ok(lib) = wc_library::Library::open(&self.dir, &self.project) {
            found.extend(lib.macros());
        }
        found
            .into_iter()
            .map(|(source, id, path)| {
                let (name, description, error) = match wc_core::macros::read_macro(&path) {
                    Ok(m) => (m.name, m.description, None),
                    Err(e) => (id.clone(), String::new(), Some(e.to_string())),
                };
                MacroEntry {
                    source,
                    path,
                    name,
                    description,
                    error,
                }
            })
            .collect()
    }

    /// Save a copied group as a macro of this project. Returns the file written.
    pub fn save_macro(&self, clip: &crate::edit::Clipboard, name: &str, description: &str) -> Result<PathBuf, String> {
        if name.trim().is_empty() {
            return Err("Give the macro a name.".into());
        }
        let id = wc_core::macros::macro_id(name);
        wc_core::macros::write_macro(&self.dir, &id, &clip.to_macro(name, description)).map_err(|e| e.to_string())
    }

    /// Insert a macro on a page where it was drawn (its anchor), like a paste. Returns the
    /// inserted items, selected so they can be moved into place.
    pub fn insert_macro(&mut self, page: usize, path: &Path) -> Result<Vec<Selection>, String> {
        if let Some(why) = self.held_reason(page) {
            return Err(why);
        }
        let m = wc_core::macros::read_macro(path).map_err(|e| e.to_string())?;
        let clip = crate::edit::Clipboard::from(m);
        let at = clip.anchor;
        Ok(self.paste(page, &clip, at))
    }

    /// Number the wires by `rule`, keeping numbers already given. One undo step; nothing is
    /// recorded when no number changes. In a shared session the pages someone else has open
    /// keep their wires as they are; their numbers are listed in `skipped`, and a later run
    /// gives their wires the numbers their connections got elsewhere.
    pub fn number_wires(&mut self, rule: wc_core::numbering::Rule) -> WireNumbering {
        self.follow_terminals();
        let mut project = self.project.clone();
        let symbols = &self.symbols;
        let done = wc_core::numbering::number_wires(
            &mut project,
            rule,
            false,
            |id| symbols.get(id).map(|s| s.role),
            |page, end| match end {
                wc_core::Endpoint::Point { at } => Some(*at),
                wc_core::Endpoint::Pin { item, pin } => {
                    let placed = page.symbol(item)?;
                    wc_pdf::pin_position(placed, symbols.get(&placed.symbol)?, pin)
                }
            },
        );
        let mut done = done;
        let mut skipped = Vec::new();
        if !self.held_by_others.is_empty() {
            for (new, old) in project.pages.iter_mut().zip(&self.project.pages) {
                if self.held_by_others.contains_key(&new.id) && new != old {
                    skipped.push(old.number.clone());
                    *new = old.clone();
                }
            }
            if !skipped.is_empty() {
                // Count again what is left: only the pages that were really changed.
                let mut numbers = std::collections::BTreeSet::new();
                done.wires = 0;
                for (new, old) in project.pages.iter().zip(&self.project.pages) {
                    for (w, was) in new.wires.iter().zip(&old.wires) {
                        if w.number != was.number {
                            done.wires += 1;
                            numbers.insert(w.number.clone());
                        }
                    }
                }
                done.connections = numbers.len();
            }
        }
        if done.wires > 0 {
            self.checkpoint();
            self.project = project;
        }
        WireNumbering { done, skipped }
    }

    /// Replace `query` with `with` in the chosen find `hits`, as one undo step
    /// ([`wc_core::find::replace`]). Nothing changes when a renamed tag clashes or is not a
    /// tag, or when the change reaches a page someone else in the session has open: the reason
    /// comes back. Returns how many values changed.
    pub fn replace_found(
        &mut self,
        hits: &[wc_core::find::Hit],
        query: &str,
        with: &str,
        opts: wc_core::find::FindOptions,
    ) -> Result<usize, String> {
        self.follow_terminals();
        let mut project = self.project.clone();
        let n = wc_core::find::replace(&mut project, hits, query, with, opts)?;
        let held: Vec<String> = project
            .pages
            .iter()
            .zip(&self.project.pages)
            .filter(|(new, old)| new != old)
            .filter_map(|(new, _)| {
                self.held_by_others
                    .get(&new.id)
                    .map(|who| format!("page {} ({who})", new.number))
            })
            .collect();
        if !held.is_empty() {
            return Err(format!(
                "Nothing was replaced: it would change {}, which someone else has open.",
                held.join(", ")
            ));
        }
        if n > 0 {
            self.checkpoint();
            self.project = project;
        }
        Ok(n)
    }

    pub fn duplicate_page(&mut self, page: usize) -> Option<usize> {
        let mut copy = self.project.pages.get(page)?.clone();
        self.checkpoint();
        copy.id = self.free_page_id();
        copy.number = self.next_page_number();
        copy.title = format!("{} (copy)", copy.title);
        // A project has one cover page; its copy is an ordinary page.
        copy.cover = false;
        let mut renamed: BTreeMap<DeviceTag, DeviceTag> = BTreeMap::new();
        for s in &mut copy.symbols {
            let Some(old) = s.tag.clone() else { continue };
            let new = match renamed.get(&old) {
                Some(t) => t.clone(),
                None => {
                    let Some(t) = self.free_tag_like(&old) else { continue };
                    let device = self.project.devices.get(&old).cloned().unwrap_or_default();
                    self.project.devices.insert(t.clone(), device);
                    renamed.insert(old, t.clone());
                    t
                }
            };
            s.tag = Some(new);
        }
        for w in &mut copy.wires {
            w.number = None;
        }
        let id = copy.id.clone();
        self.project.pages.push(copy);
        self.project.sort_pages();
        Some(self.index_of(&id))
    }

    /// A free tag with the same function, location and letter code as `like`.
    pub(crate) fn free_tag_like(&self, like: &DeviceTag) -> Option<DeviceTag> {
        let letter = like.letter_code();
        let used: std::collections::BTreeSet<DeviceTag> = self
            .project
            .tag_index()
            .into_keys()
            .chain(self.project.devices.keys().cloned())
            .collect();
        (1..10_000).find_map(|n| {
            let mut t = like.clone();
            let last = t.product.last_mut()?;
            *last = format!("{letter}{n}");
            (!used.contains(&t)).then_some(t)
        })
    }

    fn next_page_number(&self) -> String {
        let n = self
            .project
            .pages
            .iter()
            .filter_map(|p| p.number.parse::<u32>().ok())
            .max()
            .unwrap_or(0)
            + 1;
        n.to_string()
    }

    /// Set the project name, title-block fields and, when given, the whole list of locations as
    /// (path, description): empty rows are left out, a bad, doubled or pathless row is refused.
    /// Empty title-block values are dropped. One undo step.
    pub fn set_project_props(
        &mut self,
        name: &str,
        fields: &[(String, String)],
        locations: Option<&[(String, String)]>,
    ) -> Result<(), String> {
        let name = name.trim();
        if name.is_empty() {
            return Err("The project name cannot be empty.".into());
        }
        // The copy below replaces the meta: jumpers follow the change before it first.
        self.follow_terminals();
        let mut meta = self.project.meta.clone();
        meta.name = name.to_owned();
        meta.title_block = title_block_of(fields);
        if let Some(locations) = locations {
            meta.locations = wc_core::project::location_rows(locations)?;
        }
        if meta == self.project.meta {
            return Ok(());
        }
        self.checkpoint();
        self.project.meta = meta;
        Ok(())
    }

    /// Whether [`Open::cover_page`] would mark the first page as the cover: no page is marked
    /// yet, and the first one is an empty page titled Cover.
    pub fn cover_marks_first(&self) -> bool {
        self.project.cover_page().is_none()
            && self.project.pages.first().is_some_and(|first| {
                first.title.trim().eq_ignore_ascii_case("cover")
                    && first.symbols.is_empty()
                    && first.wires.is_empty()
                    && first.graphics.is_empty()
                    && first.underlay.is_none()
            })
    }

    /// The cover page's index. A project without one gets one: an empty first page titled
    /// "Cover" (as new projects started) is marked as the cover, otherwise a new cover page
    /// numbered `0` goes in front of the others. Creating or marking it is one undo step. Fails,
    /// changing nothing, when a page already uses a number that sorts as low as `0`.
    pub fn cover_page(&mut self) -> Result<usize, String> {
        if let Some(i) = self.project.cover_page() {
            return Ok(i);
        }
        if self.cover_marks_first() {
            self.checkpoint();
            if let Some(first) = self.project.pages.first_mut() {
                first.cover = true;
            }
            return Ok(0);
        }
        // `0` sorts before every other number unless one sorts as low (`0`, `00`, an empty one).
        let lowest = wc_core::project::natural_key("0");
        if let Some(p) = self
            .project
            .pages
            .iter()
            .find(|p| wc_core::project::natural_key(&p.number) <= lowest)
        {
            return Err(format!(
                "Page {:?} already comes first. Renumber it, then add the cover page in front.",
                p.number
            ));
        }
        self.checkpoint();
        let id = self.free_page_id();
        let mut page = Page::new(id.clone(), "0", "Cover");
        page.cover = true;
        if let Some(first) = self.project.pages.first() {
            page.size = first.size;
        }
        self.project.pages.push(page);
        self.project.sort_pages();
        Ok(self.index_of(&id))
    }

    /// Mark the page at `page` as the project's cover page (`on`), or as a normal page again,
    /// as one undo step. Any other cover page loses the mark: a project has one.
    pub fn set_page_cover(&mut self, page: usize, on: bool) -> Result<(), String> {
        let current = self.project.pages.get(page).ok_or("No such page.")?;
        let others = self.project.pages.iter().enumerate().any(|(i, p)| i != page && p.cover);
        if current.cover == on && !(on && others) {
            return Ok(());
        }
        self.checkpoint();
        self.project.set_cover(page, on);
        Ok(())
    }

    /// A properties form opens: its live edits will make a fresh undo step. Returns the undo
    /// mark to pass to [`Open::revert_live`].
    pub fn start_live_form(&mut self) -> usize {
        self.form_step = None;
        self.undo_mark()
    }

    /// Counts every change to the undo history. A form that shows project values compares it
    /// with the count it last saw to notice changes it did not make (an undo, the assistant).
    pub fn history_count(&self) -> u64 {
        self.history
    }

    /// The project's name and fields when the history moved since `seen` (a change the form
    /// did not make), updating `seen`; `None` when nothing else changed.
    pub fn props_if_changed(&self, seen: &mut u64) -> Option<PropsSnapshot> {
        if *seen == self.history {
            return None;
        }
        *seen = self.history;
        Some(self.props_snapshot())
    }

    /// Set the project name and fields while a form is open, so the page shows each change at
    /// once. The form's changes since [`Open::start_live_form`] stay one undo step while nothing
    /// else changes the history. An empty name is not applied (it is being typed).
    pub fn set_project_props_live(&mut self, name: &str, fields: &[(String, String)]) {
        let name = name.trim();
        let block = title_block_of(fields);
        let name_changes = !name.is_empty() && self.project.meta.name != name;
        if !name_changes && self.project.meta.title_block == block {
            return;
        }
        self.live_form_step();
        if name_changes {
            self.project.meta.name = name.to_owned();
        }
        self.project.meta.title_block = block;
    }

    /// Set the page template while its form is open, so every page shows each change at once.
    /// Like [`Open::set_project_props_live`], the form's changes stay one undo step while
    /// nothing else changes the history.
    pub fn set_template_live(&mut self, template: &PageTemplate) {
        if self.project.meta.template == *template {
            return;
        }
        self.live_form_step();
        self.project.meta.template = template.clone();
    }

    /// Before a live form's change: keep adding to the form's own undo step only while nothing
    /// else has touched the history since; after any other change (a page added, another
    /// properties edit, an undo) the form starts a new step.
    fn live_form_step(&mut self) {
        if self.form_step == Some(self.history) {
            self.dirty = true;
        } else {
            self.checkpoint();
            self.form_step = Some(self.history);
        }
    }

    /// Take back what [`Open::set_template_live`] changed since `since` (the form was
    /// cancelled), back to `before`. When the form's own undo step is the last one and holds
    /// nothing else, it is undone; otherwise the template alone is put back as a new step.
    pub fn revert_template(&mut self, since: usize, before: &PageTemplate) {
        let only_the_form = self.undo_mark() == since + 1
            && self.undo.last().is_some_and(|prev| {
                let mut p = prev.clone();
                p.meta.template.clone_from(&self.project.meta.template);
                p == self.project && prev.meta.template == *before
            });
        // When the undo cannot be done (the symbol files could not follow it), the template
        // alone is put back below, so Cancel still leaves the template as it was.
        if only_the_form && self.undo() {
            self.redo.clear();
            return;
        }
        if self.project.meta.template != *before {
            self.checkpoint();
            self.project.meta.template = before.clone();
        }
    }

    /// Whether the last undo step differs from the project only in its name and title-block
    /// fields (it is a properties form's step, with nothing else in it).
    fn last_step_changed_only_props(&self) -> bool {
        self.undo.last().is_some_and(|prev| {
            let mut p = prev.clone();
            p.meta.name.clone_from(&self.project.meta.name);
            p.meta.title_block.clone_from(&self.project.meta.title_block);
            p == self.project
        })
    }

    /// The project name and fields as they are, for [`Open::revert_live`].
    pub fn props_snapshot(&self) -> PropsSnapshot {
        PropsSnapshot {
            name: self.project.meta.name.clone(),
            title_block: self.project.meta.title_block.clone(),
        }
    }

    /// Take back what [`Open::set_project_props_live`] changed since `since` (the form was
    /// cancelled), back to `before` (a [`Open::props_snapshot`] taken when the form opened).
    /// When the form's own undo step is the last one and holds nothing else, it is undone;
    /// otherwise (the page was edited meanwhile, or the assistant folded the step into its
    /// own) only the name and fields are put back, as a new undo step.
    pub fn revert_live(&mut self, since: usize, before: &PropsSnapshot) {
        let only_the_form = self.undo_mark() == since + 1
            && self.last_step_changed_only_props()
            && self
                .undo
                .last()
                .is_some_and(|prev| prev.meta.name == before.name && prev.meta.title_block == before.title_block);
        if only_the_form {
            if self.undo() {
                self.redo.clear();
            }
            return;
        }
        if self.project.meta.name != before.name || self.project.meta.title_block != before.title_block {
            self.checkpoint();
            self.project.meta.name.clone_from(&before.name);
            self.project.meta.title_block.clone_from(&before.title_block);
        }
    }

    /// Put a wire into a cable as a core, or take it out (`None`). The cable device is created
    /// when it does not exist yet.
    pub fn set_wire_cable(&mut self, page: usize, wire: &str, cable: Option<wc_core::CableCore>) {
        let current = self
            .project
            .pages
            .get(page)
            .and_then(|p| p.wires.iter().find(|w| w.id == wire))
            .map(|w| w.cable.clone());
        if current.is_none() || current == Some(cable.clone()) {
            return;
        }
        self.checkpoint();
        if let Some(c) = &cable {
            self.project.devices.entry(c.cable.clone()).or_default();
        }
        if let Some(w) = self
            .project
            .pages
            .get_mut(page)
            .and_then(|p| p.wires.iter_mut().find(|w| w.id == wire))
        {
            w.cable = cable;
        }
    }

    /// Jumper terminals of a strip together, as one undo step. See [`wc_core::project::ProjectMeta::add_jumper`].
    pub fn add_jumper(&mut self, strip: &DeviceTag, terminals: &[String]) -> Result<(), String> {
        self.follow_terminals();
        let mut meta = self.project.meta.clone();
        meta.add_jumper(strip, terminals)?;
        if meta != self.project.meta {
            self.checkpoint();
            self.project.meta = meta;
        }
        Ok(())
    }

    /// Give the jumper that holds `terminal` of `strip` a part (taken from the library into
    /// the project when needed), or none, as one undo step.
    pub fn set_jumper_part(&mut self, strip: &DeviceTag, terminal: &str, part: Option<&str>) -> Result<(), String> {
        self.follow_terminals();
        let mut meta = self.project.meta.clone();
        if !meta.set_jumper_part(strip, terminal, part)? {
            return Ok(());
        }
        let lock = self.project.lock.clone();
        if let Some(id) = part
            && let Err(e) = self.ensure_part(id)
        {
            self.project.lock = lock;
            return Err(e);
        }
        self.checkpoint_with_lock(lock);
        self.project.meta.jumpers = meta.jumpers;
        Ok(())
    }

    /// Whether jumpering `terminals` of `strip` is worth offering: `Ok(true)` when it changes the
    /// jumpers, `Ok(false)` when they are already jumpered together, the reason when refused.
    pub fn jumper_offer(&self, strip: &DeviceTag, terminals: &[String]) -> Result<bool, String> {
        let mut after = self.project.meta.clone();
        after.add_jumper(strip, terminals)?;
        Ok(after != self.project.meta)
    }

    /// Take a terminal of a strip out of its jumper, as one undo step. Returns whether it was in one.
    pub fn remove_from_jumper(&mut self, strip: &DeviceTag, terminal: &str) -> bool {
        self.remove_from_jumpers(strip, &[terminal.to_owned()])
    }

    /// Take terminals of a strip out of their jumpers, all as one undo step. Returns whether any
    /// of them was in one.
    pub fn remove_from_jumpers(&mut self, strip: &DeviceTag, terminals: &[String]) -> bool {
        self.follow_terminals();
        let mut meta = self.project.meta.clone();
        let mut changed = false;
        for t in terminals {
            changed |= meta.remove_from_jumper(strip, t.trim());
        }
        if !changed {
            return false;
        }
        self.checkpoint();
        self.project.meta = meta;
        true
    }

    /// Set one part of the spec (section, colour or type) on wires of a page; `None` clears it.
    /// One undo step for all of them; nothing happens when no wire changes. Returns how many
    /// wires changed.
    pub fn set_wire_spec(&mut self, page: usize, wires: &[String], part: SpecPart, value: Option<String>) -> usize {
        let Some(p) = self.project.pages.get(page) else {
            return 0;
        };
        let changes = p
            .wires
            .iter()
            .filter(|w| wires.contains(&w.id) && *part.of(&w.spec) != value)
            .count();
        if changes == 0 {
            return 0;
        }
        self.checkpoint();
        if let Some(p) = self.project.pages.get_mut(page) {
            for w in p.wires.iter_mut().filter(|w| wires.contains(&w.id)) {
                *part.of_mut(&mut w.spec) = value.clone();
            }
        }
        changes
    }

    /// Cables used anywhere in the project, for the cable picker.
    pub fn cables(&self) -> Vec<DeviceTag> {
        let set: std::collections::BTreeSet<DeviceTag> = self
            .project
            .pages
            .iter()
            .flat_map(|p| p.wires.iter())
            .filter_map(|w| w.cable.as_ref().map(|c| c.cable.clone()))
            .collect();
        set.into_iter().collect()
    }

    /// Position of a pin on a page.
    pub fn pin_pos(&self, page: &Page, item: &str, pin: &str) -> Option<Point> {
        let placed = page.symbol(item)?;
        let sym = self.symbols.get(&placed.symbol)?;
        sym.pin(pin)
            .map(|p| p.at.transform(placed.rotation, placed.mirror) + placed.at)
    }

    pub fn end_pos(&self, page: &Page, e: &Endpoint) -> Option<Point> {
        match e {
            Endpoint::Point { at } => Some(*at),
            Endpoint::Pin { item, pin } => self.pin_pos(page, item, pin),
        }
    }

    /// Which way a wire leaves this end: along the pin's direction, or `None` for a free point.
    pub fn end_axis(&self, page: &Page, e: &Endpoint) -> Option<Axis> {
        let Endpoint::Pin { item, pin } = e else {
            return None;
        };
        let placed = page.symbol(item)?;
        let pin = self.symbols.get(&placed.symbol)?.pin(pin)?;
        Some(Axis::of(pin.dir, placed.rotation))
    }

    /// Redraw these wires between the same two ends in `style`, as one undo step. Returns how
    /// many wires were changed; wires with an end on a missing symbol are left alone.
    pub fn set_route(&mut self, page: usize, wires: &[String], style: Route) -> usize {
        let Some(pg) = self.project.pages.get(page) else {
            return 0;
        };
        let changes: Vec<(String, Vec<Point>)> = pg
            .wires
            .iter()
            .filter(|w| wires.contains(&w.id))
            .filter_map(|w| {
                let a = self.end_pos(pg, &w.from)?;
                let b = self.end_pos(pg, &w.to)?;
                let via = route::via(a, self.end_axis(pg, &w.from), b, self.end_axis(pg, &w.to), style);
                (via != w.via || w.route != style).then(|| (w.id.clone(), via))
            })
            .collect();
        if changes.is_empty() {
            return 0;
        }
        self.checkpoint();
        if let Some(pg) = self.project.pages.get_mut(page) {
            for (id, via) in &changes {
                if let Some(w) = pg.wires.iter_mut().find(|w| &w.id == id) {
                    w.via = via.clone();
                    w.route = style;
                }
            }
        }
        changes.len()
    }
}

/// The project name and title-block fields at one moment (see [`Open::props_snapshot`]).
#[derive(Debug, Clone, PartialEq)]
pub struct PropsSnapshot {
    pub name: String,
    pub title_block: std::collections::BTreeMap<String, String>,
}

/// The name review comments are signed with: `WIRINGCRAFT_USER`, then git's `user.name`, then
/// the login name.
/// Title-block fields from a form: trimmed, with empty keys and values dropped.
fn title_block_of(fields: &[(String, String)]) -> BTreeMap<String, String> {
    fields
        .iter()
        .map(|(k, v)| (k.trim().to_owned(), v.trim().to_owned()))
        .filter(|(k, v)| !k.is_empty() && !v.is_empty())
        .collect()
}

pub fn author_name() -> String {
    std::env::var("WIRINGCRAFT_USER")
        .ok()
        .filter(|a| !a.trim().is_empty())
        .or_else(|| {
            std::process::Command::new("git")
                .args(["config", "user.name"])
                .output()
                .ok()
                .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_owned())
                .filter(|a| !a.is_empty())
        })
        .or_else(|| std::env::var("USER").ok())
        .or_else(|| std::env::var("USERNAME").ok())
        .unwrap_or_else(|| "someone".into())
}

/// Seconds since 1970 (UTC).
pub fn now_secs() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| i64::try_from(d.as_secs()).unwrap_or(0))
        .unwrap_or(0)
}

/// Wire ends on pins of symbol `id` that `symbol` does not have: `page 3 -K1:13` each
/// ([`wc_core::Project::dangling_pins`]).
pub fn dangling_pins(project: &wc_core::Project, id: &str, symbol: &wc_core::Symbol) -> Vec<String> {
    project.dangling_pins(id, symbol)
}

/// A file's bytes before it is changed, or `None` when there is no such file. Any other
/// read error is returned: a file that is there but cannot be read must not be taken for absent
/// (and then removed when putting things back).
fn snapshot_file(path: &Path) -> Result<Option<Vec<u8>>, String> {
    match std::fs::read(path) {
        Ok(bytes) => Ok(Some(bytes)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(format!("{} cannot be read: {e}", path.display())),
    }
}

/// Put a file back as [`snapshot_file`] found it: untouched when it already holds those bytes,
/// otherwise replaced whole (written next to it, then renamed over it), so a failed write
/// leaves the file as it was rather than cut short.
fn restore_file(path: &Path, was: Option<&[u8]>) -> std::io::Result<()> {
    match was {
        Some(bytes) => {
            if std::fs::read(path).is_ok_and(|now| now == bytes) {
                return Ok(());
            }
            let mut tmp = path.as_os_str().to_owned();
            tmp.push(".tmp");
            let tmp = PathBuf::from(tmp);
            std::fs::write(&tmp, bytes)?;
            std::fs::rename(&tmp, path)
        }
        None => match std::fs::remove_file(path) {
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            r => r,
        },
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
pub(crate) mod tests {
    use super::*;
    use wc_core::project::LibrarySource;

    pub(crate) fn project() -> (tempfile::TempDir, Open) {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("p");
        let mut p = store::create(&dir, "Test").unwrap();
        let lib = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../library");
        p.meta.libraries.push(LibrarySource {
            name: "starter".into(),
            kind: "folder".into(),
            location: lib.display().to_string(),
            reference: None,
        });
        p.pages.push(Page::new("p0002", "2", "Control"));
        store::save(&dir, &p).unwrap();
        let open = Open::load(&dir).unwrap();
        (tmp, open)
    }

    #[test]
    fn a_part_file_that_cannot_be_read_is_not_taken_for_absent() {
        let tmp = tempfile::tempdir().unwrap();
        let missing = tmp.path().join("missing.json");
        assert_eq!(super::snapshot_file(&missing).unwrap(), None);
        // A directory where the file should be: there, but not readable as a file.
        let unreadable = tmp.path().join("part.json");
        std::fs::create_dir(&unreadable).unwrap();
        let err = super::snapshot_file(&unreadable).unwrap_err();
        assert!(err.contains("cannot be read"), "{err}");
    }

    #[test]
    fn a_part_file_is_put_back_whole_or_not_at_all() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("part.json");
        std::fs::write(&path, b"new").unwrap();
        // The replacement cannot be written next to the file: the file keeps what it had.
        std::fs::create_dir(tmp.path().join("part.json.tmp")).unwrap();
        assert!(super::restore_file(&path, Some(b"old")).is_err());
        assert_eq!(std::fs::read(&path).unwrap(), b"new");
        // A file that already holds the bytes is not written at all.
        assert!(super::restore_file(&path, Some(b"new")).is_ok());
        std::fs::remove_dir(tmp.path().join("part.json.tmp")).unwrap();
        super::restore_file(&path, Some(b"old")).unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), b"old");
        super::restore_file(&path, None).unwrap();
        assert!(!path.exists());
        super::restore_file(&path, None).unwrap();
    }

    #[test]
    fn undo_puts_back_the_symbol_version_a_library_update_replaced() {
        let (_tmp, mut o) = project();
        o.place(1, "wc.coil", Point::new(20.0, 20.0)).unwrap();
        let old = o.symbols["wc.coil"].clone();
        let mut new = old.clone();
        new.name = "Coil, changed".into();
        o.checkpoint();
        store::write_vendored_symbol(&o.dir, &new).unwrap();
        o.project.lock.symbols.get_mut("wc.coil").unwrap().hash = new.hash();
        o.reload_symbols();
        assert_eq!(o.symbols["wc.coil"], new);

        assert!(o.undo());
        assert_eq!(o.symbols["wc.coil"], old, "the cache follows the lock");
        assert_eq!(
            store::read_vendored_symbol(&o.dir, "wc.coil").unwrap(),
            old,
            "and the file"
        );
        assert!(o.redo());
        assert_eq!(o.symbols["wc.coil"], new);
        assert_eq!(store::read_vendored_symbol(&o.dir, "wc.coil").unwrap(), new);
    }

    #[test]
    fn redo_puts_back_a_new_version_of_a_symbol_no_page_uses() {
        let (_tmp, mut o) = project();
        o.ensure_symbol("wc.coil").unwrap();
        let old = o.symbols["wc.coil"].clone();
        let mut new = old.clone();
        new.name = "Coil, changed".into();
        o.checkpoint();
        store::write_vendored_symbol(&o.dir, &new).unwrap();
        o.project.lock.symbols.get_mut("wc.coil").unwrap().hash = new.hash();
        o.reload_symbols();
        assert_eq!(o.symbols["wc.coil"], new, "kept though no page uses it");
        assert!(o.undo());
        assert_eq!(o.symbols["wc.coil"], old);
        assert!(o.redo());
        assert_eq!(o.symbols["wc.coil"], new);
        assert_eq!(store::read_vendored_symbol(&o.dir, "wc.coil").unwrap(), new);
        assert_eq!(o.project.lock.symbols["wc.coil"].hash, new.hash());
    }

    #[test]
    fn a_pinned_symbol_whose_copy_is_gone_is_put_back_in_the_version_it_pins() {
        let (tmp, mut o) = project();
        o.ensure_symbol("wc.coil").unwrap();
        let old = o.symbols["wc.coil"].clone();
        o.save().unwrap();
        // Its copy gone when the project is opened: pinned, but not loaded.
        std::fs::remove_file(store::vendor_path(&o.dir, ItemKind::Symbol, "wc.coil").unwrap()).unwrap();
        let mut o = Open::load(&o.dir).unwrap();
        assert!(!o.symbols.contains_key("wc.coil"));
        // A newer version wins for the id; the source the pin names still has the old one.
        let mut new = old.clone();
        new.name = "Coil, changed".into();
        library_first(&mut o, tmp.path(), &new);
        o.place(1, "wc.coil", Point::new(20.0, 20.0)).unwrap();
        assert_eq!(o.symbols["wc.coil"], old, "placed in the version pinned");
        assert_eq!(store::read_vendored_symbol(&o.dir, "wc.coil").unwrap(), old);
        assert_eq!(o.project.lock.symbols["wc.coil"].hash, old.hash());
    }

    #[test]
    fn a_pinned_symbol_loaded_in_another_version_is_not_placed() {
        let (_tmp, mut o) = project();
        o.ensure_symbol("wc.coil").unwrap();
        let old = o.symbols["wc.coil"].clone();
        o.save().unwrap();
        // The file is changed outside the app: the project still pins the old version.
        let mut new = old.clone();
        new.name = "Coil, changed".into();
        store::write_vendored_symbol(&o.dir, &new).unwrap();
        let mut o = Open::load(&o.dir).unwrap();
        assert_eq!(o.symbols["wc.coil"], new, "loaded as the file has it");
        let err = o.place(1, "wc.coil", Point::new(20.0, 20.0)).unwrap_err();
        assert!(err.contains("not the version the project pins"), "{err}");
    }

    #[test]
    fn a_pinned_symbol_is_put_back_from_any_library_that_has_its_version() {
        let (tmp, mut o) = project();
        o.ensure_symbol("wc.coil").unwrap();
        let old = o.symbols["wc.coil"].clone();
        o.save().unwrap();
        std::fs::remove_file(store::vendor_path(&o.dir, ItemKind::Symbol, "wc.coil").unwrap()).unwrap();
        let mut o = Open::load(&o.dir).unwrap();
        // The pin names a library that has moved on ("extra"), and another one wins for the id
        // ("mine"); only the last one ("starter") still has the pinned version.
        let mut new = old.clone();
        new.name = "Coil, changed".into();
        library_first(&mut o, tmp.path(), &new);
        let extra = tmp.path().join("extra");
        std::fs::create_dir_all(extra.join("symbols")).unwrap();
        std::fs::write(extra.join("symbols/wc.coil.json"), serde_json::to_string(&new).unwrap()).unwrap();
        o.project.meta.libraries.insert(
            1,
            LibrarySource {
                name: "extra".into(),
                kind: "folder".into(),
                location: extra.display().to_string(),
                reference: None,
            },
        );
        o.project.lock.symbols.get_mut("wc.coil").unwrap().source = "extra".into();
        o.ensure_symbol("wc.coil").unwrap();
        assert_eq!(o.symbols["wc.coil"], old);
        assert_eq!(store::read_vendored_symbol(&o.dir, "wc.coil").unwrap(), old);
    }

    #[test]
    fn a_pinned_symbol_no_library_has_in_its_version_is_not_placed() {
        let (tmp, mut o) = project();
        o.ensure_symbol("wc.coil").unwrap();
        let old = o.symbols["wc.coil"].clone();
        o.save().unwrap();
        std::fs::remove_file(store::vendor_path(&o.dir, ItemKind::Symbol, "wc.coil").unwrap()).unwrap();
        let mut o = Open::load(&o.dir).unwrap();
        let mut new = old.clone();
        new.name = "Coil, changed".into();
        library_first(&mut o, tmp.path(), &new);
        // The pin names the newer library only: nowhere has the pinned version.
        o.project.lock.symbols.get_mut("wc.coil").unwrap().source = "mine".into();
        o.project.meta.libraries.retain(|l| l.name == "mine");
        let lock = o.project.lock.clone();
        let err = o.place(1, "wc.coil", Point::new(20.0, 20.0)).unwrap_err();
        assert!(
            err.contains("mine: another version"),
            "what each library had is said: {err}"
        );
        assert_eq!(o.project.lock, lock, "the pin is kept");
        assert!(
            !store::vendor_path(&o.dir, ItemKind::Symbol, "wc.coil")
                .unwrap()
                .exists()
        );
    }

    /// A library folder named "mine", put ahead of the others, holding `sym`.
    fn library_first(o: &mut Open, root: &Path, sym: &Symbol) -> wc_library::Library {
        let mine = root.join("mine");
        std::fs::create_dir_all(mine.join("symbols")).unwrap();
        std::fs::write(
            mine.join(format!("symbols/{}.json", sym.id)),
            serde_json::to_string(sym).unwrap(),
        )
        .unwrap();
        o.project.meta.libraries.insert(
            0,
            LibrarySource {
                name: "mine".into(),
                kind: "folder".into(),
                location: mine.display().to_string(),
                reference: None,
            },
        );
        wc_library::Library::open(&o.dir, &o.project).unwrap()
    }

    /// Put a changed copy of a vendored part into the "mine" library folder.
    fn changed_part_in_mine(o: &Open, root: &Path, pid: &str) -> wc_core::Part {
        let mut part = store::read_vendored_part(&o.dir, pid).unwrap();
        part.description = "Changed in the library".into();
        std::fs::create_dir_all(root.join("mine/parts")).unwrap();
        std::fs::write(
            root.join(format!("mine/parts/{pid}.json")),
            serde_json::to_string(&part).unwrap(),
        )
        .unwrap();
        part
    }

    #[test]
    fn a_jumper_part_is_one_undo_step_with_its_pin() {
        let (_tmp, mut o) = project();
        let x1 = DeviceTag::parse("-X1").unwrap();
        for (x, tag) in [(20.0, "-X1.1"), (40.0, "-X1.2")] {
            let id = o.place(1, "wc.terminal", Point::new(x, 20.0)).unwrap();
            let s = o.project.pages[1].symbols.iter_mut().find(|s| s.id == id).unwrap();
            s.tag = Some(DeviceTag::parse(tag).unwrap());
        }
        o.add_jumper(&x1, &["1".into(), "2".into()]).unwrap();
        let pid = "example.ex-tb-2-5";
        let lock = o.project.lock.clone();
        assert!(!lock.parts.contains_key(pid));
        o.set_jumper_part(&x1, "2", Some(pid)).unwrap();
        assert!(o.project.lock.parts.contains_key(pid));
        assert!(o.undo());
        assert_eq!(o.project.lock, lock, "Undo takes the pin back with the part");
        assert!(o.project.meta.jumpers[0].part.is_none());
        assert!(o.redo());
        assert!(o.project.lock.parts.contains_key(pid), "Redo brings it back");
        assert_eq!(o.project.meta.jumpers[0].part.as_deref(), Some(pid));
    }

    #[test]
    fn a_part_copy_without_its_pin_is_pinned_as_it_is_not_replaced() {
        let (tmp, mut o) = project();
        let pid = "example.ex-ct-9-24dc";
        let tag = DeviceTag::parse("-K1").unwrap();
        o.set_device_part(&tag, Some(pid)).unwrap();
        // The pin is gone (an undone import left the copy), and the library has a newer one.
        let copy = store::read_vendored_part(&o.dir, pid).unwrap();
        o.project.lock.parts.remove(pid);
        o.parts.remove(pid);
        changed_part_in_mine(&o, tmp.path(), pid);
        o.place(1, "wc.coil", Point::new(20.0, 20.0)).unwrap();
        let coil = o.symbols["wc.coil"].clone();
        library_first(&mut o, tmp.path(), &coil);
        let other = DeviceTag::parse("-K2").unwrap();
        o.set_device_part(&other, Some(pid)).unwrap();
        assert_eq!(
            store::read_vendored_part(&o.dir, pid).unwrap(),
            copy,
            "the copy is kept"
        );
        assert_eq!(o.project.lock.parts[pid].hash, copy.hash(), "and pinned as it is");

        // A pinned part whose copy is missing is not replaced by another library version: the
        // pinned one is put back from a library that still has it, though the pin names
        // another and a newer one comes first.
        o.parts.remove(pid);
        std::fs::remove_file(store::vendor_path(&o.dir, ItemKind::Part, pid).unwrap()).unwrap();
        let third = DeviceTag::parse("-K3").unwrap();
        o.set_device_part(&third, Some(pid)).unwrap();
        assert_eq!(store::read_vendored_part(&o.dir, pid).unwrap(), copy);
        o.parts.remove(pid);
        std::fs::remove_file(store::vendor_path(&o.dir, ItemKind::Part, pid).unwrap()).unwrap();
        // Pinned to a source that still has the pinned version, though another one comes first
        // with a newer one: the copy comes from the source the pin names.
        let pin = o.project.lock.parts[pid].clone();
        o.project.lock.parts.get_mut(pid).unwrap().source = "starter".into();
        let sixth = DeviceTag::parse("-K6").unwrap();
        o.set_device_part(&sixth, Some(pid)).unwrap();
        assert_eq!(store::read_vendored_part(&o.dir, pid).unwrap(), copy);
        o.parts.remove(pid);
        std::fs::remove_file(store::vendor_path(&o.dir, ItemKind::Part, pid).unwrap()).unwrap();
        o.project.lock.parts.insert(pid.into(), pin);
        // Pinned to a source that has moved on, while the source that comes first still has the
        // pinned version: the copy comes from that one, and the pin stays as it is.
        let pin = o.project.lock.parts[pid].clone();
        assert_eq!(pin.source, "mine");
        o.project.meta.libraries.rotate_left(1);
        let seventh = DeviceTag::parse("-K7").unwrap();
        o.set_device_part(&seventh, Some(pid)).unwrap();
        assert_eq!(store::read_vendored_part(&o.dir, pid).unwrap(), copy);
        assert_eq!(o.project.lock.parts[pid], pin);
        o.project.meta.libraries.rotate_right(1);
        o.parts.remove(pid);
        std::fs::remove_file(store::vendor_path(&o.dir, ItemKind::Part, pid).unwrap()).unwrap();
        // With the pinned version back in the library, the copy is put back and the pin kept.
        let pin = o.project.lock.parts[pid].clone();
        wc_library::write_part(&tmp.path().join("mine"), &copy).unwrap();
        o.set_device_part(&DeviceTag::parse("-K8").unwrap(), Some(pid)).unwrap();
        assert_eq!(store::read_vendored_part(&o.dir, pid).unwrap(), copy);
        assert_eq!(o.project.lock.parts[pid], pin);

        // A copy that is not the pinned version is not used.
        let mut edited = copy.clone();
        edited.description = "Edited by hand".into();
        store::write_vendored_part(&o.dir, &edited).unwrap();
        o.parts.remove(pid);
        let fifth = DeviceTag::parse("-K5").unwrap();
        let err = o.set_device_part(&fifth, Some(pid)).unwrap_err();
        assert!(err.contains("not the version the project pins"), "{err}");
        store::write_vendored_part(&o.dir, &copy).unwrap();

        // A copy without its pin whose library file cannot be read says why.
        o.project.lock.parts.remove(pid);
        o.parts.remove(pid);
        std::fs::write(tmp.path().join(format!("mine/parts/{pid}.json")), "{ not json").unwrap();
        let fourth = DeviceTag::parse("-K4").unwrap();
        let err = o.set_device_part(&fourth, Some(pid)).unwrap_err();
        assert!(
            err.contains("cannot be read") && err.contains("line 1"),
            "the parse error is kept: {err}"
        );
    }

    #[test]
    fn an_update_refused_for_a_dropped_pin_writes_no_part_either() {
        let (tmp, mut o) = project();
        let s1 = o.place(1, "wc.coil", Point::new(20.0, 20.0)).unwrap();
        let s2 = o.place(1, "wc.coil", Point::new(60.0, 20.0)).unwrap();
        let pin = |item: &str| Endpoint::Pin {
            item: item.to_owned(),
            pin: "A2".into(),
        };
        o.project.pages[1].wires.push(Wire {
            id: "w1".into(),
            from: pin(&s1),
            to: pin(&s2),
            via: Vec::new(),
            number: None,
            potential: None,
            cable: None,
            route: Default::default(),
            spec: Default::default(),
        });
        let pid = "example.ex-ct-9-24dc";
        o.ensure_part(pid).unwrap();
        let part_file = store::vendor_path(&o.dir, ItemKind::Part, pid).unwrap();
        let part_before = std::fs::read(&part_file).unwrap();
        changed_part_in_mine(&o, tmp.path(), pid);
        let mut new = o.symbols["wc.coil"].clone();
        new.pins.retain(|p| p.name != "A2");
        let lib = library_first(&mut o, tmp.path(), &new);
        let lock = o.project.lock.clone();
        let err = o.update_library(&lib, None).unwrap_err();
        assert!(err.starts_with("nothing was updated") && err.contains(":A2"), "{err}");
        assert_eq!(
            std::fs::read(&part_file).unwrap(),
            part_before,
            "the part file is untouched"
        );
        assert_eq!(o.project.lock, lock);
    }

    #[test]
    fn a_change_taken_back_leaves_no_part_file_and_keeps_undo_marks_in_step() {
        let (_tmp, mut o) = project();
        o.place(1, "wc.coil", Point::new(20.0, 20.0)).unwrap();
        let (undo_mark, can_undo) = (o.undo_mark(), o.can_undo());
        let pid = "example.ex-ct-9-24dc";
        let part_file = store::vendor_path(&o.dir, ItemKind::Part, pid).unwrap();
        // A refused change that took a part from the library: its file goes with it.
        let mark = o.mark();
        assert_eq!(o.undo_mark(), undo_mark, "the set-aside steps still count");
        o.ensure_part(pid).unwrap();
        o.checkpoint();
        assert!(part_file.exists());
        o.back_to(mark).unwrap();
        assert!(!part_file.exists(), "no unpinned part file is left");
        assert!(!o.project.lock.parts.contains_key(pid));
        assert_eq!((o.undo_mark(), o.can_undo()), (undo_mark, can_undo));
        // Taken again later, it is pinned (and so sent to the session).
        o.ensure_part(pid).unwrap();
        assert!(o.project.lock.parts.contains_key(pid));
        // A file that was there before stays, even when the change only read it.
        o.parts.remove(pid);
        let mark = o.mark();
        o.ensure_part(pid).unwrap();
        o.back_to(mark).unwrap();
        assert!(part_file.exists(), "a file that was there before stays");
        // An unpinned copy left behind (a failed removal) is pinned when it is next used.
        o.project.lock.parts.remove(pid);
        o.parts.remove(pid);
        o.ensure_part(pid).unwrap();
        assert!(
            o.project.lock.parts.contains_key(pid),
            "the leftover copy is pinned again"
        );
        // A vendor folder that cannot be checked (here: a link out of the project) is reported,
        // and nothing in it is removed.
        #[cfg(unix)]
        {
            let parts = o.dir.join(store::VENDOR_DIR).join(ItemKind::Part.dir());
            let outside = _tmp.path().join("outside-parts");
            std::fs::rename(&parts, &outside).unwrap();
            std::os::unix::fs::symlink(&outside, &parts).unwrap();
            let mark = o.mark();
            let err = o.back_to(mark).unwrap_err();
            assert!(
                err.contains("not checked") && err.contains("outside the project"),
                "{err}"
            );
            assert!(part_file.exists(), "nothing removed");
            std::fs::remove_file(&parts).unwrap();
            std::fs::rename(&outside, &parts).unwrap();
        }
        // A mark taken inside a kept change (a form it opened) means the same step afterwards.
        let mark = o.mark();
        let inside = o.start_live_form();
        o.keep(mark);
        assert_eq!(inside, o.undo_mark());
    }

    #[test]
    fn an_update_takes_only_the_items_it_looked_at() {
        let (tmp, mut o) = project();
        o.place(1, "wc.coil", Point::new(20.0, 20.0)).unwrap();
        let pid = "example.ex-ct-9-24dc";
        o.ensure_part(pid).unwrap();
        changed_part_in_mine(&o, tmp.path(), pid);
        let old = o.symbols["wc.coil"].clone();
        // The library's coil file names another id: not an update when the update looks.
        let mut wrong = old.clone();
        wrong.id = "wc.other".into();
        let lib = library_first(&mut o, tmp.path(), &wrong);
        let file = tmp.path().join("mine/symbols/wc.coil.json");
        std::fs::write(&file, serde_json::to_string(&wrong).unwrap()).unwrap();
        // Fixed and changed right after the backups are taken.
        let mut new = old.clone();
        new.name = "Coil, changed".into();
        o.rewrite_after_snapshot = Some((file, serde_json::to_string(&new).unwrap()));
        let done = o.update_library(&lib, None).unwrap();
        assert_eq!(done, vec![(ItemKind::Part, pid.to_owned())]);
        assert_eq!(
            store::read_vendored_symbol(&o.dir, "wc.coil").unwrap(),
            old,
            "the coil it did not look at is not taken"
        );
        assert_eq!(o.project.lock.symbols["wc.coil"].hash, old.hash());
    }

    #[test]
    fn an_update_takes_no_newer_version_than_it_looked_at_and_keeps_redo() {
        let (tmp, mut o) = project();
        o.place(1, "wc.coil", Point::new(20.0, 20.0)).unwrap();
        let pid = "example.ex-ct-9-24dc";
        o.ensure_part(pid).unwrap();
        // An edit undone before the update: Redo must still bring it back after a failed one.
        o.checkpoint();
        o.project.pages[1].title = "Renamed".into();
        assert!(o.undo());
        assert!(o.can_redo());
        let part_file = store::vendor_path(&o.dir, ItemKind::Part, pid).unwrap();
        let part_before = std::fs::read(&part_file).unwrap();
        let mut seen = changed_part_in_mine(&o, tmp.path(), pid);
        let old = o.symbols["wc.coil"].clone();
        let mut new = old.clone();
        new.name = "Coil, changed".into();
        let lib = library_first(&mut o, tmp.path(), &new);
        // The part changes again right after the backups are taken.
        seen.description = "Changed again".into();
        o.rewrite_after_snapshot = Some((
            tmp.path().join(format!("mine/parts/{pid}.json")),
            serde_json::to_string(&seen).unwrap(),
        ));
        let lock = o.project.lock.clone();
        let err = o.update_library(&lib, None).unwrap_err();
        assert!(
            err.contains("changed in the library while it was being updated"),
            "{err}"
        );
        assert_eq!(
            std::fs::read(&part_file).unwrap(),
            part_before,
            "the part file is as it was"
        );
        assert_eq!(
            store::read_vendored_symbol(&o.dir, "wc.coil").unwrap(),
            old,
            "and the coil"
        );
        assert_eq!(o.project.lock, lock);
        assert!(o.can_redo(), "the earlier Redo is kept");
        assert!(o.redo());
        assert_eq!(o.project.pages[1].title, "Renamed");
    }

    #[test]
    fn a_failed_update_leaves_a_full_undo_history_and_a_saved_project_as_they_were() {
        let (tmp, mut o) = project();
        o.place(1, "wc.coil", Point::new(20.0, 20.0)).unwrap();
        let pid = "example.ex-ct-9-24dc";
        o.ensure_part(pid).unwrap();
        for n in 0..UNDO_STEPS {
            o.checkpoint();
            o.project.pages[1].title = format!("Step {n}");
        }
        let mut seen = changed_part_in_mine(&o, tmp.path(), pid);
        let coil = o.symbols["wc.coil"].clone();
        let lib = library_first(&mut o, tmp.path(), &coil);
        o.save().unwrap();
        let (undo, history, dropped) = (o.undo.clone(), o.history, o.undo_dropped);
        seen.description = "Changed again".into();
        o.rewrite_after_snapshot = Some((
            tmp.path().join(format!("mine/parts/{pid}.json")),
            serde_json::to_string(&seen).unwrap(),
        ));
        assert!(o.update_library(&lib, None).is_err());
        assert_eq!(o.undo, undo, "the oldest Undo step is still there");
        assert_eq!((o.history, o.undo_dropped), (history, dropped));
        assert!(!o.dirty, "nothing to save");
    }

    #[test]
    fn an_update_whose_later_write_fails_takes_back_the_symbol_written_before_it() {
        let (tmp, mut o) = project();
        o.place(1, "wc.coil", Point::new(20.0, 20.0)).unwrap();
        let pid = "example.ex-ct-9-24dc";
        o.ensure_part(pid).unwrap();
        changed_part_in_mine(&o, tmp.path(), pid);
        let old = o.symbols["wc.coil"].clone();
        let mut new = old.clone();
        new.name = "Coil, changed".into();
        let lib = library_first(&mut o, tmp.path(), &new);
        // Once the backups are taken, a folder where the part file goes: symbols are taken
        // first, then this write fails.
        let part_file = store::vendor_path(&o.dir, ItemKind::Part, pid).unwrap();
        o.block_after_snapshot = Some(part_file.clone());
        let lock = o.project.lock.clone();
        let err = o.update_library(&lib, None).unwrap_err();
        assert!(
            !err.starts_with("nothing was updated"),
            "the update itself failed: {err}"
        );
        assert_eq!(o.project.lock, lock, "the lock is as before");
        assert_eq!(o.symbols["wc.coil"], old);
        assert_eq!(
            store::read_vendored_symbol(&o.dir, "wc.coil").unwrap(),
            old,
            "and the symbol file"
        );
        assert!(!o.can_redo());
    }

    #[test]
    fn an_update_whose_part_file_cannot_be_read_changes_nothing_and_keeps_the_file() {
        let (tmp, mut o) = project();
        o.place(1, "wc.coil", Point::new(20.0, 20.0)).unwrap();
        let pid = "example.ex-ct-9-24dc";
        o.ensure_part(pid).unwrap();
        changed_part_in_mine(&o, tmp.path(), pid);
        let old = o.symbols["wc.coil"].clone();
        let mut new = old.clone();
        new.name = "Coil, changed".into();
        let lib = library_first(&mut o, tmp.path(), &new);
        // The part file is a plain, removable file whose read fails.
        let part_file = store::vendor_path(&o.dir, ItemKind::Part, pid).unwrap();
        let bytes = std::fs::read(&part_file).unwrap();
        o.unreadable = Some(part_file.clone());
        let lock = o.project.lock.clone();
        let mark = o.undo_mark();
        let err = o.update_library(&lib, None).unwrap_err();
        assert!(
            err.starts_with("nothing was updated") && err.contains("cannot be read"),
            "{err}"
        );
        assert_eq!(std::fs::read(&part_file).unwrap(), bytes, "the file is kept as it was");
        assert_eq!(o.project.lock, lock);
        assert_eq!(
            store::read_vendored_symbol(&o.dir, "wc.coil").unwrap(),
            old,
            "no symbol written"
        );
        assert_eq!(o.undo_mark(), mark, "no undo step");
    }

    #[test]
    fn undo_after_reopening_puts_back_a_locked_symbol_no_page_uses() {
        let (tmp, mut o) = project();
        o.ensure_symbol("wc.coil").unwrap();
        o.save().unwrap();
        let old = store::read_vendored_symbol(&o.dir, "wc.coil").unwrap();
        let mut o = Open::load(&o.dir).unwrap();
        let mut new = old.clone();
        new.name = "Coil, changed".into();
        let lib = library_first(&mut o, tmp.path(), &new);
        let done = o.update_library(&lib, None).unwrap();
        assert!(done.contains(&(ItemKind::Symbol, "wc.coil".to_owned())));
        assert_eq!(store::read_vendored_symbol(&o.dir, "wc.coil").unwrap(), new);
        assert!(o.undo());
        assert_eq!(o.symbols["wc.coil"], old);
        assert_eq!(store::read_vendored_symbol(&o.dir, "wc.coil").unwrap(), old);
    }

    #[test]
    fn a_library_update_that_drops_a_wired_pin_is_taken_back() {
        let (tmp, mut o) = project();
        let s1 = o.place(1, "wc.coil", Point::new(20.0, 20.0)).unwrap();
        let s2 = o.place(1, "wc.coil", Point::new(60.0, 20.0)).unwrap();
        let pin = |item: &str| Endpoint::Pin {
            item: item.to_owned(),
            pin: "A2".into(),
        };
        o.project.pages[1].wires.push(Wire {
            id: "w1".into(),
            from: pin(&s1),
            to: pin(&s2),
            via: Vec::new(),
            number: None,
            potential: None,
            cable: None,
            route: Default::default(),
            spec: Default::default(),
        });
        let old = o.symbols["wc.coil"].clone();
        let mut new = old.clone();
        new.pins.retain(|p| p.name != "A2");
        let lib = library_first(&mut o, tmp.path(), &new);
        let before = o.project.clone();
        let err = o.update_library(&lib, None).unwrap_err();
        assert!(err.contains(":A2"), "{err}");
        assert_eq!(o.project, before, "nothing was updated");
        assert_eq!(o.symbols["wc.coil"], old);
        assert_eq!(store::read_vendored_symbol(&o.dir, "wc.coil").unwrap(), old);
        assert!(!o.can_redo(), "no step is left to redo into the broken version");
    }

    #[test]
    fn a_symbol_taken_back_by_undo_is_pinned_again_when_it_is_placed() {
        let (_tmp, mut o) = project();
        let lib = wc_library::Library::open(&o.dir, &o.project).unwrap();
        o.take_saved_symbol(&lib, "wc.lamp").unwrap();
        assert!(o.symbols.contains_key("wc.lamp"));
        assert!(o.undo());
        assert!(!o.project.lock.symbols.contains_key("wc.lamp"));
        assert!(!o.symbols.contains_key("wc.lamp"), "not locked and not used: let go");
        assert!(o.redo(), "the version is kept for Redo");
        assert!(o.symbols.contains_key("wc.lamp") && o.project.lock.symbols.contains_key("wc.lamp"));
        assert!(o.undo());
        o.place(1, "wc.lamp", Point::new(20.0, 20.0)).unwrap();
        assert!(
            o.project.lock.symbols.contains_key("wc.lamp"),
            "every placed symbol is pinned"
        );
    }

    #[test]
    fn an_undo_whose_symbol_version_is_gone_says_why_the_file_could_not_be_read() {
        let (_tmp, mut o) = project();
        o.place(1, "wc.coil", Point::new(20.0, 20.0)).unwrap();
        let mut new = o.symbols["wc.coil"].clone();
        new.name = "Coil, changed".into();
        o.checkpoint();
        store::write_vendored_symbol(&o.dir, &new).unwrap();
        o.project.lock.symbols.get_mut("wc.coil").unwrap().hash = new.hash();
        o.reload_symbols();
        o.symbol_versions.clear();
        let file = store::vendor_path(&o.dir, ItemKind::Symbol, "wc.coil").unwrap();
        std::fs::write(&file, "not a symbol").unwrap();
        assert!(!o.undo());
        let err = o.take_undo_error().unwrap_or_default();
        let reason = err.split("cannot be read: ").nth(1).unwrap_or_default();
        assert!(!reason.trim().is_empty(), "the parse error follows: {err}");
        let parse = store::read_vendored_symbol(&o.dir, "wc.coil").unwrap_err().to_string();
        assert!(err.ends_with(&parse), "{err} / {parse}");
    }

    #[test]
    fn a_rollback_that_fails_is_written_again_by_the_next_undo() {
        let (_tmp, mut o) = project();
        o.place(1, "wc.coil", Point::new(20.0, 20.0)).unwrap();
        o.place(1, "wc.lamp", Point::new(60.0, 20.0)).unwrap();
        let (coil, lamp) = (o.symbols["wc.coil"].clone(), o.symbols["wc.lamp"].clone());
        o.checkpoint();
        let mut coil2 = coil.clone();
        coil2.name = "Coil, changed".into();
        let mut lamp2 = lamp.clone();
        lamp2.name = "Lamp, changed".into();
        for s in [&coil2, &lamp2] {
            store::write_vendored_symbol(&o.dir, s).unwrap();
            o.project.lock.symbols.get_mut(&s.id).unwrap().hash = s.hash();
        }
        o.reload_symbols();
        // Undo: the coil file is written, the lamp write fails, and putting the coil back fails.
        o.fail_symbol_writes = [false, true, true].into();
        assert!(!o.undo());
        let err = o.take_undo_error().unwrap_or_default();
        assert!(err.contains("could not be restored: wc.coil"), "{err}");
        assert_eq!(o.symbols["wc.coil"], coil2, "memory follows the lock");
        assert_eq!(
            store::read_vendored_symbol(&o.dir, "wc.coil").unwrap(),
            coil,
            "the file does not"
        );
        // A step whose lock names what memory already holds still writes the coil file again.
        o.checkpoint();
        assert!(o.undo());
        assert_eq!(store::read_vendored_symbol(&o.dir, "wc.coil").unwrap(), coil2);
        // And the step that failed now goes through.
        assert!(o.undo());
        assert_eq!(store::read_vendored_symbol(&o.dir, "wc.coil").unwrap(), coil);
        assert_eq!(store::read_vendored_symbol(&o.dir, "wc.lamp").unwrap(), lamp);
    }

    #[test]
    fn an_undo_whose_symbol_file_cannot_be_written_changes_nothing() {
        let (_tmp, mut o) = project();
        o.place(1, "wc.coil", Point::new(20.0, 20.0)).unwrap();
        let mut new = o.symbols["wc.coil"].clone();
        new.name = "Coil, changed".into();
        o.checkpoint();
        store::write_vendored_symbol(&o.dir, &new).unwrap();
        o.project.lock.symbols.get_mut("wc.coil").unwrap().hash = new.hash();
        o.reload_symbols();
        let after = o.project.clone();
        // A folder where the symbol file goes: the write fails, even for root.
        let file = store::vendor_path(&o.dir, ItemKind::Symbol, "wc.coil").unwrap();
        std::fs::remove_file(&file).unwrap();
        std::fs::create_dir(&file).unwrap();
        assert!(!o.undo());
        assert!(o.take_undo_error().is_some_and(|e| e.contains("wc.coil")));
        assert_eq!(o.project, after, "the project stays as it was");
        assert_eq!(o.symbols["wc.coil"], new);
        assert!(o.can_undo() && !o.can_redo(), "the step is still there to undo");
    }

    #[test]
    fn saving_a_symbol_leaves_a_part_with_the_same_id_alone() {
        let (tmp, mut o) = project();
        o.place(1, "wc.coil", Point::new(20.0, 20.0)).unwrap();
        let pid = "example.ex-ct-9-24dc";
        o.ensure_part(pid).unwrap();
        let part = store::read_vendored_part(&o.dir, pid).unwrap();
        let locked = o.project.lock.parts[pid].clone();
        // A library folder ahead of the starter one with a changed part and a symbol, same id.
        let mine = tmp.path().join("mine");
        std::fs::create_dir_all(mine.join("symbols")).unwrap();
        std::fs::create_dir_all(mine.join("parts")).unwrap();
        let mut changed = part.clone();
        changed.description = "Changed in the library".into();
        std::fs::write(
            mine.join(format!("parts/{pid}.json")),
            serde_json::to_string(&changed).unwrap(),
        )
        .unwrap();
        let mut sym = o.symbols["wc.coil"].clone();
        sym.id = pid.into();
        std::fs::write(
            mine.join(format!("symbols/{pid}.json")),
            serde_json::to_string(&sym).unwrap(),
        )
        .unwrap();
        o.project.meta.libraries.insert(
            0,
            LibrarySource {
                name: "mine".into(),
                kind: "folder".into(),
                location: mine.display().to_string(),
                reference: None,
            },
        );
        let lib = wc_library::Library::open(&o.dir, &o.project).unwrap();
        o.take_saved_symbol(&lib, pid).unwrap();
        assert!(o.project.lock.symbols.contains_key(pid));
        assert_eq!(o.project.lock.parts[pid], locked, "the part's lock entry is unchanged");
        assert_eq!(store::read_vendored_part(&o.dir, pid).unwrap(), part, "and its copy");
    }

    #[test]
    fn a_symbol_save_that_fails_leaves_the_history_alone() {
        let (_tmp, mut o) = project();
        o.place(1, "wc.coil", Point::new(20.0, 20.0)).unwrap();
        let before = o.project.clone();
        let steps = o.undo.len();
        let lib = wc_library::Library::open(&o.dir, &o.project).unwrap();
        assert!(o.take_saved_symbol(&lib, "no.such-symbol").is_err());
        assert_eq!(o.project, before, "the project stays as it was");
        assert_eq!(o.undo.len(), steps, "and no empty undo step was added");
    }

    fn fields(pairs: &[(&str, &str)]) -> Vec<(String, String)> {
        pairs.iter().map(|(k, v)| ((*k).to_owned(), (*v).to_owned())).collect()
    }

    #[cfg(unix)]
    #[test]
    fn a_logo_is_not_copied_through_a_pictures_link_out_of_the_project() {
        let (tmp, mut o) = project();
        let outside = tmp.path().join("outside");
        std::fs::create_dir_all(&outside).unwrap();
        std::os::unix::fs::symlink(&outside, o.dir.join(wc_core::Logo::DIR)).unwrap();
        let src = tmp.path().join("logo.png");
        let mut png = Vec::new();
        image::DynamicImage::ImageRgb8(image::RgbImage::from_pixel(2, 2, image::Rgb([1, 2, 3])))
            .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
            .unwrap();
        std::fs::write(&src, png).unwrap();
        let err = o.set_logo_from(&src).unwrap_err();
        assert!(err.contains("out of the project"), "{err}");
        assert_eq!(
            std::fs::read_dir(&outside).unwrap().count(),
            0,
            "nothing written outside"
        );
        assert!(o.project.meta.logo.is_none());
    }

    #[test]
    fn a_logo_that_cannot_be_shown_keeps_the_reason() {
        let (_tmp, mut o) = project();
        let ctx = egui::Context::default();
        assert!(o.picture_texture(&ctx, "pictures/none.png").is_none());
        assert_eq!(
            o.picture_problems.get("pictures/none.png").map(String::as_str),
            Some("the file is missing")
        );
        std::fs::create_dir_all(o.dir.join("pictures")).unwrap();
        std::fs::write(o.dir.join("pictures/bad.png"), "not a picture").unwrap();
        assert!(o.picture_texture(&ctx, "pictures/bad.png").is_none());
        assert!(o.picture_problems["pictures/bad.png"].starts_with("not a picture"));
        assert!(o.picture_texture(&ctx, "../x.png").is_none());
        assert!(o.picture_problems["../x.png"].contains("not inside the project"));
    }

    #[test]
    fn a_picked_logo_is_copied_into_the_project_as_one_undo_step() {
        let (tmp, mut o) = project();
        let png = |rgb: [u8; 3]| {
            let mut out = Vec::new();
            image::DynamicImage::ImageRgb8(image::RgbImage::from_pixel(3, 2, image::Rgb(rgb)))
                .write_to(&mut std::io::Cursor::new(&mut out), image::ImageFormat::Png)
                .unwrap();
            out
        };
        let src = tmp.path().join("Our Logo.png");
        std::fs::write(&src, png([255, 0, 0])).unwrap();
        assert_eq!(o.set_logo_from(&src).unwrap(), "pictures/Our_Logo.png");
        assert_eq!(
            std::fs::read(o.dir.join("pictures/Our_Logo.png")).unwrap(),
            png([255, 0, 0])
        );
        // The same picture again reuses the file; another one of the same name gets its own.
        assert_eq!(o.set_logo_from(&src).unwrap(), "pictures/Our_Logo.png");
        std::fs::write(&src, png([0, 0, 255])).unwrap();
        assert_eq!(o.set_logo_from(&src).unwrap(), "pictures/Our_Logo-2.png");
        assert_eq!(
            std::fs::read(o.dir.join("pictures/Our_Logo.png")).unwrap(),
            png([255, 0, 0])
        );
        // Moving it on the cover is kept when another picture is picked.
        o.checkpoint();
        o.set_logo_place(Point::new(10.0, 10.0), Point::new(40.0, 30.0));
        std::fs::write(&src, png([0, 255, 0])).unwrap();
        o.set_logo_from(&src).unwrap();
        let logo = o.project.meta.logo.clone().unwrap();
        assert_eq!(logo.cover, Some((Point::new(10.0, 10.0), Point::new(40.0, 30.0))));
        assert!(o.undo());
        assert_eq!(o.project.meta.logo.as_ref().unwrap().file, "pictures/Our_Logo-2.png");
        o.remove_logo();
        assert!(o.project.meta.logo.is_none());
        assert!(o.undo(), "removing is one step too");
        assert!(o.project.meta.logo.is_some());
        // Saved with the project and loaded again.
        o.save().unwrap();
        let again = Open::load(&o.dir).unwrap();
        assert_eq!(again.project.meta.logo, o.project.meta.logo);
        // Not a picture: refused, nothing changes.
        let txt = tmp.path().join("logo.txt");
        std::fs::write(&txt, "hello").unwrap();
        let before = o.project.meta.logo.clone();
        assert!(o.set_logo_from(&txt).unwrap_err().contains("PNG or JPEG"));
        assert_eq!(o.project.meta.logo, before);
    }

    #[test]
    fn a_change_made_without_changes_leaves_no_trace_in_the_undo_history() {
        let (_tmp, mut o) = project();
        o.checkpoint();
        o.project.pages[0].title = "First".into();
        assert!(o.undo());
        o.dirty = false;
        let (project, mark) = (o.project.clone(), o.undo_mark());
        assert!(o.can_redo());
        o.without_changes(|o| {
            o.checkpoint();
            o.project.pages[0].title = "Second".into();
        });
        assert_eq!(o.project, project);
        assert_eq!(o.undo_mark(), mark, "no undo step");
        assert!(o.can_redo(), "redo kept");
        assert!(!o.dirty, "not marked as changed");
    }

    #[test]
    fn the_cover_form_shows_changes_as_typed_and_keeps_them_as_one_undo_step() {
        let (_tmp, mut o) = project();
        assert_eq!(o.cover_page(), Ok(0), "a new project's first page is its cover");
        assert!(!o.can_undo(), "nothing to create");
        o.start_live_form();
        for typed in ["A", "AC", "ACME"] {
            o.set_project_props_live("Test", &fields(&[("customer", typed)]));
            assert_eq!(o.project.meta.title_block["customer"], typed, "shown at once");
        }
        o.set_project_props_live("", &fields(&[("customer", "ACME")]));
        assert_eq!(o.project.meta.name, "Test", "an empty name is still being typed");
        o.set_project_props_live("Pumps", &fields(&[("customer", "ACME"), ("revision", "")]));
        assert_eq!(o.project.meta.name, "Pumps");
        assert_eq!(o.project.meta.title_block.len(), 1, "empty fields are dropped");
        assert!(o.undo());
        assert_eq!(o.project.meta.name, "Test");
        assert!(
            o.project.meta.title_block.is_empty(),
            "one undo takes the whole form back"
        );
        assert!(!o.can_undo());

        // Cancel takes the typed changes back and leaves nothing to redo.
        let since = o.start_live_form();
        let before = o.props_snapshot();
        o.set_project_props_live("Test", &fields(&[("customer", "Other")]));
        o.revert_live(since, &before);
        assert!(o.project.meta.title_block.is_empty());
        assert!(!o.can_redo());
        o.revert_live(since, &before);
        assert!(!o.can_undo(), "nothing changed, nothing undone");
    }

    #[test]
    fn the_template_form_is_one_undo_step_and_cancel_puts_it_back() {
        let (_tmp, mut o) = project();
        o.start_live_form();
        let before = o.project.meta.template.clone();
        let mut draft = before.clone();
        for columns in [11, 12] {
            draft.columns = columns;
            o.set_template_live(&draft);
            assert_eq!(o.project.meta.template.columns, columns, "shown at once");
        }
        draft.boxes[0].label = "Client".into();
        o.set_template_live(&draft);
        // Saved with the project and read back.
        o.save().unwrap();
        let text = std::fs::read_to_string(o.dir.join("project.toml")).unwrap();
        assert!(text.contains("[template]") && text.contains("Client"), "{text}");
        assert_eq!(store::load(&o.dir).unwrap().meta.template, draft);
        assert!(o.undo(), "one step");
        assert_eq!(o.project.meta.template, before);
        assert!(!o.can_undo());

        // Cancel takes the form's changes back and leaves nothing to redo.
        let since2 = o.start_live_form();
        draft.columns = 8;
        o.set_template_live(&draft);
        o.revert_template(since2, &before);
        assert_eq!(o.project.meta.template, before);
        assert!(!o.can_redo() && !o.can_undo());
    }

    /// The cover form's flow in `main.rs`: it refreshes its buffer when the history count moved
    /// without it, and records the count after each of its own edits.
    #[test]
    fn the_form_buffer_follows_an_undo_so_a_later_edit_keeps_it() {
        let (_tmp, mut o) = project();
        o.start_live_form();
        let mut buffer = fields(&[("customer", "A")]);
        o.set_project_props_live("Test", &buffer);
        let mut seen = o.history_count();
        assert!(
            o.props_if_changed(&mut seen).is_none(),
            "the form's own edit is not news"
        );
        // Undo from the toolbar while the form is open: the form reloads what it shows.
        assert!(o.undo());
        let now = o.props_if_changed(&mut seen).unwrap();
        buffer = now.title_block.into_iter().collect();
        // Editing another field now does not bring customer A back.
        buffer.push(("revision".into(), "B".into()));
        o.set_project_props_live("Test", &buffer);
        assert!(!o.project.meta.title_block.contains_key("customer"));
        assert_eq!(o.project.meta.title_block["revision"], "B");
    }

    #[test]
    fn cancel_restores_opening_props_after_undo_and_other_property_edits() {
        let (_tmp, mut o) = project();
        o.set_project_props("Opening", &fields(&[("customer", "Opening")]), None)
            .unwrap();
        let since = o.start_live_form();
        let before = o.props_snapshot();
        o.set_project_props_live("Form", &fields(&[("customer", "Form")]));
        assert!(o.undo());
        assert!(o.undo());
        o.set_project_props("First", &fields(&[("customer", "First")]), None)
            .unwrap();
        o.set_project_props("Second", &fields(&[("customer", "Second")]), None)
            .unwrap();
        // The last step looks like a form step but is not the form's: Cancel still restores
        // what the form opened with.
        assert_eq!(o.undo_mark(), since + 1);
        assert!(o.last_step_changed_only_props());
        o.revert_live(since, &before);
        assert_eq!(o.props_snapshot(), before);
    }

    #[test]
    fn form_edits_after_an_undo_stay_one_step() {
        let (_tmp, mut o) = project();
        o.checkpoint();
        o.project.pages[0].title = "Edited".into();
        o.start_live_form();
        assert!(o.undo());
        o.set_project_props_live("Test", &fields(&[("customer", "A")]));
        o.set_project_props_live("Test", &fields(&[("customer", "B")]));
        assert!(o.undo());
        assert!(o.project.meta.title_block.is_empty(), "one Undo takes both edits back");
        assert!(!o.can_undo());
    }

    #[test]
    fn a_form_edit_after_another_change_is_its_own_undo_step() {
        let (_tmp, mut o) = project();
        o.start_live_form();
        o.set_project_props_live("Test", &fields(&[("customer", "A")]));
        // Another change while the form is open.
        o.checkpoint();
        o.project.pages[0].title = "Added meanwhile".into();
        o.set_project_props_live("Test", &fields(&[("customer", "B")]));
        // OK, then Undo: only the second form edit goes back.
        assert!(o.undo());
        assert_eq!(o.project.meta.title_block["customer"], "A");
        assert_eq!(o.project.pages[0].title, "Added meanwhile");

        // A properties change made another way in between is its own step too.
        o.form_step = None;
        o.start_live_form();
        o.set_project_props_live("Test", &fields(&[("customer", "A")]));
        o.set_project_props("Test", &fields(&[("customer", "X")]), None)
            .unwrap();
        o.set_project_props_live("Test", &fields(&[("customer", "B")]));
        assert!(o.undo());
        assert_eq!(o.project.meta.title_block["customer"], "X");
    }

    #[test]
    fn cancel_puts_the_fields_back_when_the_form_step_is_not_the_last_or_was_folded_away() {
        let (_tmp, mut o) = project();
        // A page edit made while the form is open stays; only the fields go back.
        let since = o.start_live_form();
        let before = o.props_snapshot();
        o.set_project_props_live("Test", &fields(&[("customer", "ACME")]));
        o.checkpoint();
        o.project.pages[0].title = "Edited meanwhile".into();
        o.revert_live(since, &before);
        assert!(o.project.meta.title_block.is_empty());
        assert_eq!(o.project.pages[0].title, "Edited meanwhile");

        // An assistant turn that began before the form folds the form's step into its own.
        let turn = o.undo_mark();
        o.checkpoint();
        o.project.pages[0].title = "By the assistant".into();
        let since = o.start_live_form();
        let before = o.props_snapshot();
        o.set_project_props_live("Test", &fields(&[("customer", "ACME")]));
        o.squash_undo(turn);
        assert!(o.undo_mark() <= since);
        o.revert_live(since, &before);
        assert!(o.project.meta.title_block.is_empty(), "the form's change is taken back");
        assert_eq!(o.project.pages[0].title, "By the assistant", "the assistant's stays");
    }

    #[test]
    fn a_project_without_a_cover_gets_one_in_front() {
        let (_tmp, mut o) = project();
        o.project.pages[0].cover = false;
        o.project.pages[0].title = "Overview".into();
        let i = o.cover_page().unwrap();
        let cover = &o.project.pages[i];
        assert!(cover.cover);
        assert_eq!((i, cover.number.as_str(), cover.title.as_str()), (0, "0", "Cover"));
        assert_eq!(o.project.pages.len(), 3);
        assert_eq!(o.cover_page(), Ok(0), "found again, not made twice");
        assert!(o.undo());
        assert_eq!(o.project.pages.len(), 2, "making it is one undo step");

        // An empty first page titled "Cover" (how projects started) becomes the cover.
        o.project.pages[0].title = "Cover".into();
        assert_eq!(o.cover_page(), Ok(0));
        assert_eq!(o.project.pages.len(), 2);
        assert!(o.project.pages[0].cover);

        // Its copy is a normal page: a project keeps one cover.
        let copy = o.duplicate_page(0).unwrap();
        assert!(!o.project.pages[copy].cover);
    }

    #[test]
    fn a_first_page_titled_cover_with_a_drawing_on_it_is_kept_and_a_cover_goes_in_front() {
        let (_tmp, mut o) = project();
        o.project.pages[0].cover = false;
        o.project.pages[0].graphics.push(wc_core::geom::Shape::Line {
            a: Point::new(0.0, 0.0),
            b: Point::new(10.0, 0.0),
        });
        let i = o.cover_page().unwrap();
        assert_eq!((i, o.project.pages[i].number.as_str()), (0, "0"));
        assert!(!o.project.pages[1].cover, "the drawn page stays as it was");
    }

    #[test]
    fn a_page_numbered_zero_keeps_its_place_and_no_cover_is_made() {
        let (_tmp, mut o) = project();
        o.project.pages[0].cover = false;
        o.project.pages[0].title = "Index".into();
        o.project.pages[0].number = "0".into();
        o.project.sort_pages();
        let before = o.project.clone();
        assert!(o.cover_page().unwrap_err().contains("\"0\""));
        assert_eq!(o.project, before);
        assert!(!o.can_undo(), "nothing changed");
    }

    #[test]
    fn changing_the_route_redraws_selected_wires_in_one_undo_step() {
        let (_tmp, mut o) = project();
        let page = o.project.pages.iter().position(|p| p.id == "p0002").unwrap();
        let k1 = o.place(page, "wc.coil", Point::new(20.0, 20.0)).unwrap();
        let k2 = o.place(page, "wc.coil", Point::new(60.0, 80.0)).unwrap();
        let pin = |item: &str, pin: &str| Endpoint::Pin {
            item: item.into(),
            pin: pin.into(),
        };
        o.add_wire(page, pin(&k1, "A2"), pin(&k2, "A1"), Vec::new());
        o.add_wire(
            page,
            pin(&k2, "A2"),
            Endpoint::Point {
                at: Point::new(100.0, 140.0),
            },
            Vec::new(),
        );
        let ids: Vec<String> = o.project.pages[page].wires.iter().map(|w| w.id.clone()).collect();
        let before = o.project.pages.clone();
        assert_eq!(o.set_route(page, &ids, Route::Sloped), 2);
        for w in &o.project.pages[page].wires {
            assert_eq!(w.route, Route::Sloped);
            let a = o.end_pos(&o.project.pages[page], &w.from).unwrap();
            let b = o.end_pos(&o.project.pages[page], &w.to).unwrap();
            let expected = route::via(
                a,
                o.end_axis(&o.project.pages[page], &w.from),
                b,
                o.end_axis(&o.project.pages[page], &w.to),
                Route::Sloped,
            );
            assert_eq!(w.via, expected);
            assert!(!w.via.is_empty(), "the ends are not lined up, so the wire bends");
        }
        assert_eq!(o.set_route(page, &ids, Route::Sloped), 0, "nothing left to change");
        assert!(o.undo());
        assert_eq!(o.project.pages, before, "one undo puts both wires back");
    }

    #[test]
    fn a_rotated_coil_leaves_its_pins_sideways() {
        let (_tmp, mut o) = project();
        let page = o.project.pages.iter().position(|p| p.id == "p0002").unwrap();
        let k1 = o.place(page, "wc.coil", Point::new(20.0, 20.0)).unwrap();
        let end = Endpoint::Pin {
            item: k1.clone(),
            pin: "A1".into(),
        };
        assert_eq!(o.end_axis(&o.project.pages[page], &end), Some(Axis::Vertical));
        o.project.pages[page].symbols[0].rotation = Rotation::R90;
        assert_eq!(o.end_axis(&o.project.pages[page], &end), Some(Axis::Horizontal));
        let free = Endpoint::Point {
            at: Point::new(0.0, 0.0),
        };
        assert_eq!(o.end_axis(&o.project.pages[page], &free), None);
    }

    #[test]
    fn auto_wiring_is_part_of_the_placing_undo_step() {
        let (_tmp, mut o) = project();
        let page = o.project.pages.iter().position(|p| p.id == "p0002").unwrap();
        o.place(page, "wc.contact.no", Point::new(50.0, 20.0)).unwrap();
        let before = o.project.pages.clone();
        let coil = o.place(page, "wc.coil", Point::new(50.0, 60.0)).unwrap();
        let items = std::iter::once(coil.clone()).collect();
        assert_eq!(o.auto_wire(page, &items), 1);
        let w = &o.project.pages[page].wires[0];
        assert_eq!(
            (&w.from, &w.to),
            (
                &Endpoint::Pin {
                    item: coil,
                    pin: "A1".into()
                },
                &Endpoint::Pin {
                    item: "s1".into(),
                    pin: "14".into()
                }
            )
        );
        assert_eq!(o.auto_wire(page, &items), 0, "a wired pin is not wired again");
        assert!(o.undo());
        assert_eq!(o.project.pages, before, "one undo removes the symbol and its wire");
    }

    #[test]
    fn placing_vendors_the_symbol_and_tags_the_device() {
        let (_tmp, mut o) = project();
        assert!(o.library.iter().any(|(id, _)| id == "wc.coil"));
        let page = o.project.pages.iter().position(|p| p.id == "p0002").unwrap();
        let id = o.place(page, "wc.coil", Point::new(50.0, 50.0)).unwrap();
        assert!(o.symbols.contains_key("wc.coil"));
        assert!(o.project.lock.symbols.contains_key("wc.coil"));
        let tag = o.project.pages[page].symbol(&id).unwrap().tag.clone().unwrap();
        assert_eq!(tag.to_string(), "-K1");
        // The next one on a page with =A+B tags inherits them and skips the used number.
        o.project.pages[page].symbols[0].tag = Some(DeviceTag::parse("=PUMP+CAB1-K1").unwrap());
        let id2 = o.place(page, "wc.coil", Point::new(80.0, 50.0)).unwrap();
        assert_eq!(
            o.project.pages[page]
                .symbol(&id2)
                .unwrap()
                .tag
                .as_ref()
                .unwrap()
                .to_string(),
            "=PUMP+CAB1-K2"
        );
    }

    #[test]
    fn undo_redo_and_delete_drop_attached_wires() {
        let (_tmp, mut o) = project();
        let page = o.project.pages.iter().position(|p| p.id == "p0002").unwrap();
        let a = o.place(page, "wc.coil", Point::new(50.0, 50.0)).unwrap();
        o.add_wire(
            page,
            Endpoint::Pin {
                item: a.clone(),
                pin: "A1".into(),
            },
            Endpoint::Point {
                at: Point::new(50.0, 20.0),
            },
            vec![],
        );
        assert_eq!(o.project.pages[page].wires.len(), 1);
        o.delete_many(page, &[Selection::Symbol(a)]);
        assert!(o.project.pages[page].symbols.is_empty());
        assert!(o.project.pages[page].wires.is_empty());
        assert!(o.undo());
        assert_eq!(o.project.pages[page].wires.len(), 1);
        assert!(o.redo());
        assert!(o.project.pages[page].wires.is_empty());
        // Saving writes the page and clears the dirty flag.
        o.save().unwrap();
        assert!(!o.dirty);
        let again = Open::load(&o.dir).unwrap();
        assert!(again.project.pages[page].symbols.is_empty());
    }

    #[test]
    fn hit_testing_finds_symbols_wires_and_pins() {
        let (_tmp, mut o) = project();
        let page = o.project.pages.iter().position(|p| p.id == "p0002").unwrap();
        let a = o.place(page, "wc.coil", Point::new(50.0, 50.0)).unwrap();
        o.add_wire(
            page,
            Endpoint::Point {
                at: Point::new(100.0, 20.0),
            },
            Endpoint::Point {
                at: Point::new(100.0, 80.0),
            },
            vec![],
        );
        let pg = o.project.pages[page].clone();
        assert_eq!(
            o.hit(&pg, Point::new(50.0, 50.0), 1.0),
            Some(Selection::Symbol(a.clone()))
        );
        assert!(matches!(
            o.hit(&pg, Point::new(100.5, 40.0), 1.0),
            Some(Selection::Wire(_))
        ));
        assert_eq!(o.hit(&pg, Point::new(150.0, 150.0), 1.0), None);
        let a1 = o.pin_pos(&pg, &a, "A1").unwrap();
        let (item, pin, _) = o.pin_near(&pg, a1 + Point::new(0.5, 0.5), 2.0).unwrap();
        assert_eq!((item.as_str(), pin.as_str()), (a.as_str(), "A1"));
    }

    #[test]
    fn a_page_someone_else_has_open_is_neither_deleted_nor_written_on() {
        // A Delete page or Text dialog confirmed after the page became held changes nothing.
        let (_tmp, mut o) = project();
        let id = o.project.pages[1].id.clone();
        o.held_by_others.insert(id, "Ola".into());
        let before = o.project.clone();
        let text = wc_core::geom::Shape::Text {
            at: wc_core::geom::Point::new(10.0, 10.0),
            text: "Note".into(),
            height: 3.5,
            align: wc_core::geom::Align::Left,
            angle: 0.0,
        };
        assert!(o.delete_page(1).is_err_and(|e| e.contains("Ola")));
        assert!(o.add_graphic(1, text.clone()).is_none());
        assert!(o.held_reason(1).is_some_and(|e| e.contains("Ola")));
        // Nor renumbered by moving its neighbour past it, nor given a macro or a paste.
        assert_eq!(o.move_page(0, false), None);
        let path = o.dir.join("missing.wcmacro");
        assert!(o.insert_macro(1, &path).is_err_and(|e| e.contains("Ola")));
        let clip = crate::edit::Clipboard {
            graphics: vec![text.clone()],
            ..Default::default()
        };
        assert!(o.paste(1, &clip, wc_core::geom::Point::new(0.0, 0.0)).is_empty());
        assert_eq!(o.project, before);
        assert!(!o.can_undo(), "nothing was recorded");
        // The other page is not held.
        assert!(o.held_reason(0).is_none());
        assert!(o.add_graphic(0, text).is_some());
    }

    #[test]
    fn a_pages_own_title_fields_are_one_undo_step_and_saved() {
        let (_tmp, mut o) = project();
        o.project.meta.title_block.insert("revision".into(), "A".into());
        let p = o.project.pages[1].clone();
        let mut fields = BTreeMap::new();
        fields.insert("revision".to_owned(), " B ".to_owned());
        fields.insert("checked_by".to_owned(), "OH".to_owned());
        let i = o
            .set_page_props(1, &p.number, "Renamed", p.size, p.overview, Some(&fields))
            .unwrap();
        let page = &o.project.pages[i];
        assert_eq!(page.title_field(&o.project.meta, "revision"), "B");
        assert_eq!(page.title_block["checked_by"], "OH");
        o.save().unwrap();
        let disk = store::load(&o.dir).unwrap();
        assert_eq!(
            disk.pages[i].title_block, o.project.pages[i].title_block,
            "saved in the page file"
        );

        // Clearing a field falls back to the project's; a key left out is kept.
        let mut clear = BTreeMap::new();
        clear.insert("revision".to_owned(), String::new());
        o.set_page_props(i, &p.number, "Renamed", p.size, p.overview, Some(&clear))
            .unwrap();
        let page = &o.project.pages[i];
        assert_eq!(page.title_field(&o.project.meta, "revision"), "A");
        assert_eq!(page.title_block.get("checked_by").map(String::as_str), Some("OH"));
        // The same values again change nothing and record nothing.
        let mark = o.undo_mark();
        o.set_page_props(i, &p.number, "Renamed", p.size, p.overview, Some(&clear))
            .unwrap();
        assert_eq!(o.undo_mark(), mark);

        // Each change was one undo step: title and fields go back together.
        assert!(o.undo());
        assert_eq!(
            o.project.pages[i].title_block.get("revision").map(String::as_str),
            Some("B")
        );
        assert!(o.undo());
        assert_eq!(o.project.pages[i].title, p.title);
        assert!(o.project.pages[i].title_block.is_empty());
    }

    #[test]
    fn pages_can_be_renamed_moved_duplicated_and_deleted() {
        let (_tmp, mut o) = project();
        assert_eq!(o.project.pages.len(), 2);
        // Renumbering resorts; a taken number is refused.
        assert!(o.set_page_props(0, "2", "Cover", PageSize::A3, false, None).is_err());
        let i = o
            .set_page_props(0, "10", "Cover sheet", PageSize::A4, true, None)
            .unwrap();
        assert!(o.project.pages[i].overview);
        assert_eq!(i, 1);
        assert_eq!(o.project.pages[1].title, "Cover sheet");
        assert_eq!(o.project.pages[1].size, PageSize::A4);
        // Moving swaps numbers with the neighbour.
        assert_eq!(o.move_page(1, true), Some(0));
        assert_eq!(o.project.pages[0].number, "2");
        assert_eq!(o.project.pages[0].id, "p0001");
        assert_eq!(o.move_page(0, true), None);
        // Duplicating gives the copy new tags for the same devices.
        let page = o.project.pages.iter().position(|p| p.id == "p0002").unwrap();
        let a = o.place(page, "wc.coil", Point::new(50.0, 50.0)).unwrap();
        let old = o.project.pages[page].symbol(&a).unwrap().tag.clone().unwrap();
        o.project.devices.get_mut(&old).unwrap().description = "Pump contactor".into();
        let c = o.duplicate_page(page).unwrap();
        let new = o.project.pages[c].symbols[0].tag.clone().unwrap();
        assert_ne!(new, old);
        assert_eq!(new.letter_code(), old.letter_code());
        assert_eq!(o.project.devices[&new].description, "Pump contactor");
        assert_eq!(o.project.pages.len(), 3);
        // Delete down to one page, which stays.
        o.delete_page(c).unwrap();
        o.delete_page(0).unwrap();
        assert!(o.delete_page(0).is_err());
        assert_eq!(o.project.pages.len(), 1);
        o.save().unwrap();
        assert_eq!(Open::load(&o.dir).unwrap().project.pages.len(), 1);
    }

    #[test]
    fn wires_join_and_leave_cables() {
        let (_tmp, mut o) = project();
        let page = o.project.pages.iter().position(|p| p.id == "p0002").unwrap();
        let free = |x| Endpoint::Point {
            at: Point::new(x, 10.0),
        };
        o.add_wire(page, free(10.0), free(20.0), vec![]);
        let id = o.project.pages[page].wires[0].id.clone();
        let w1 = DeviceTag::parse("-W1").unwrap();
        o.set_wire_cable(
            page,
            &id,
            Some(wc_core::CableCore {
                cable: w1.clone(),
                core: "BN".into(),
            }),
        );
        assert!(o.project.devices.contains_key(&w1));
        assert_eq!(o.cables(), vec![w1]);
        o.set_wire_cable(page, &id, None);
        assert!(o.cables().is_empty());
        assert!(o.undo());
        assert_eq!(
            o.project.pages[page].wires[0].cable.as_ref().map(|c| c.core.as_str()),
            Some("BN")
        );
    }

    #[test]
    fn jumpers_are_added_and_removed_in_one_undo_step_each() {
        let (_tmp, mut o) = project();
        let x1 = DeviceTag::parse("-X1").unwrap();
        let mark = o.undo_mark();
        o.add_jumper(&x1, &["1".into(), "2".into()]).unwrap();
        assert_eq!(o.undo_mark(), mark + 1);
        o.add_jumper(&x1, &["2".into(), "1".into()]).unwrap();
        assert_eq!(o.undo_mark(), mark + 1, "the same jumper again is no change");
        assert!(o.add_jumper(&x1, &["1".into()]).is_err());
        o.save().unwrap();
        assert_eq!(Open::load(&o.dir).unwrap().project.meta.jumpers.len(), 1);
        assert!(o.remove_from_jumper(&x1, "1"));
        assert!(o.project.meta.jumpers.is_empty());
        assert!(!o.remove_from_jumper(&x1, "1"));
        assert!(o.undo());
        assert_eq!(o.project.meta.jumpers.len(), 1);
    }

    #[test]
    fn the_jumper_button_is_offered_only_when_it_changes_something() {
        let (_tmp, mut o) = project();
        let x1 = DeviceTag::parse("-X1").unwrap();
        let v = |t: &[&str]| t.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        assert!(
            o.jumper_offer(&x1, &v(&["1", "1"])).is_err(),
            "two placements of one terminal"
        );
        assert_eq!(o.jumper_offer(&x1, &v(&["1", "2"])), Ok(true));
        o.add_jumper(&x1, &v(&["1", "2"])).unwrap();
        assert_eq!(o.jumper_offer(&x1, &v(&["1", "2"])), Ok(false), "already jumpered");
        assert_eq!(o.jumper_offer(&x1, &v(&["2", "3"])), Ok(true));
    }

    #[test]
    fn several_wires_get_a_section_colour_and_type_in_one_step() {
        let (_tmp, mut o) = project();
        let page = o.project.pages.iter().position(|p| p.id == "p0002").unwrap();
        let free = |x| Endpoint::Point {
            at: Point::new(x, 10.0),
        };
        o.add_wire(page, free(10.0), free(20.0), vec![]);
        o.add_wire(page, free(30.0), free(40.0), vec![]);
        let ids: Vec<String> = o.project.pages[page].wires.iter().map(|w| w.id.clone()).collect();
        let mark = o.undo_mark();
        assert_eq!(o.set_wire_spec(page, &ids, SpecPart::Colour, Some("BU".into())), 2);
        assert_eq!(o.undo_mark(), mark + 1, "one undo step for both");
        assert_eq!(
            o.set_wire_spec(page, &ids, SpecPart::Colour, Some("BU".into())),
            0,
            "no change, no step"
        );
        assert_eq!(o.undo_mark(), mark + 1);
        o.set_wire_spec(page, &ids[..1], SpecPart::Section, Some("1.5 mm²".into()));
        let w = &o.project.pages[page].wires;
        assert_eq!(
            (w[0].spec.colour.as_deref(), w[1].spec.colour.as_deref()),
            (Some("BU"), Some("BU"))
        );
        assert_eq!(
            (w[0].spec.section.as_deref(), w[1].spec.section.as_deref()),
            (Some("1.5 mm²"), None)
        );
        o.save().unwrap();
        let back = Open::load(&o.dir).unwrap();
        assert_eq!(
            back.project.pages[page].wires[0].spec,
            o.project.pages[page].wires[0].spec
        );
        // Clearing a part the wires differ in clears it on all of them.
        assert_eq!(o.set_wire_spec(page, &ids, SpecPart::Section, None), 1);
        assert!(o.project.pages[page].wires.iter().all(|w| w.spec.section.is_none()));
        assert!(o.undo() && o.undo() && o.undo());
        assert!(o.project.pages[page].wires.iter().all(|w| w.spec.is_empty()));
    }

    #[test]
    fn a_jumper_gets_a_part_from_the_library_as_one_undo_step() {
        let (_tmp, mut o) = project();
        let x1 = DeviceTag::parse("-X1").unwrap();
        o.add_jumper(&x1, &["1".into(), "2".into()]).unwrap();
        assert!(o.set_jumper_part(&x1, "9", Some("example.ex-tb-2-5")).is_err());
        assert!(o.set_jumper_part(&x1, "1", Some("no.such.part")).is_err());
        assert_eq!(o.project.meta.jumpers[0].part, None);
        o.set_jumper_part(&x1, "2", Some("example.ex-tb-2-5")).unwrap();
        assert_eq!(o.project.meta.jumpers[0].part.as_deref(), Some("example.ex-tb-2-5"));
        assert!(o.parts.contains_key("example.ex-tb-2-5"), "loaded for the reports");
        o.save().unwrap();
        let back = Open::load(&o.dir).unwrap();
        assert!(back.parts.contains_key("example.ex-tb-2-5"), "loaded again on opening");
        o.set_jumper_part(&x1, "1", None).unwrap();
        assert_eq!(o.project.meta.jumpers[0].part, None);
        assert!(o.undo());
        assert_eq!(o.project.meta.jumpers[0].part.as_deref(), Some("example.ex-tb-2-5"));
    }

    #[test]
    fn devices_get_parts_from_the_library_and_own_properties() {
        let (_tmp, mut o) = project();
        let page = o.project.pages.iter().position(|p| p.id == "p0002").unwrap();
        let a = o.place(page, "wc.coil", Point::new(50.0, 50.0)).unwrap();
        let tag = o.project.pages[page].symbol(&a).unwrap().tag.clone().unwrap();
        let (id, label) = o
            .part_library
            .iter()
            .find(|(id, _)| id.contains("ct-9"))
            .cloned()
            .unwrap();
        assert!(!label.is_empty());
        o.set_device_part(&tag, Some(&id)).unwrap();
        assert_eq!(o.project.devices[&tag].part.as_deref(), Some(id.as_str()));
        assert!(o.parts.contains_key(&id));
        assert!(o.set_device_part(&tag, Some("no.such.part")).is_err());
        o.set_device_property(&tag, "coil voltage", "24 V DC");
        assert_eq!(o.project.devices[&tag].properties["coil voltage"], "24 V DC");
        o.set_device_property(&tag, "coil voltage", "");
        assert!(o.project.devices[&tag].properties.is_empty());
        o.set_device_part(&tag, None).unwrap();
        assert_eq!(o.project.devices[&tag].part, None);
        assert!(o.undo());
        assert_eq!(o.project.devices[&tag].part.as_deref(), Some(id.as_str()));
    }

    #[test]
    fn devices_get_accessories_with_quantities() {
        let (_tmp, mut o) = project();
        let page = o.project.pages.iter().position(|p| p.id == "p0002").unwrap();
        let a = o.place(page, "wc.coil", Point::new(50.0, 50.0)).unwrap();
        let tag = o.project.pages[page].symbol(&a).unwrap().tag.clone().unwrap();
        let id = o.part_library.first().map(|(id, _)| id.clone()).unwrap();
        o.set_device_accessories(
            &tag,
            vec![DevicePart {
                part: id.clone(),
                qty: 0,
            }],
        )
        .unwrap();
        assert_eq!(
            o.project.devices[&tag].accessories,
            [DevicePart {
                part: id.clone(),
                qty: 1
            }]
        );
        assert!(o.parts.contains_key(&id), "taken into the project");
        assert!(o.project.used_parts().contains(&id));
        assert!(
            o.set_device_accessories(
                &tag,
                vec![DevicePart {
                    part: "no.such.part".into(),
                    qty: 1
                }]
            )
            .is_err()
        );
        assert_eq!(
            o.project.devices[&tag].accessories.len(),
            1,
            "a failed change keeps what was there"
        );
        assert!(o.undo());
        assert!(o.project.devices.get(&tag).is_none_or(|d| d.accessories.is_empty()));
    }

    #[test]
    fn project_props_drop_empty_title_block_fields() {
        let (_tmp, mut o) = project();
        assert!(o.set_project_props(" ", &[], None).is_err());
        o.set_project_props(
            "Pump station",
            &[("customer".into(), "Acme".into()), ("revision".into(), " ".into())],
            None,
        )
        .unwrap();
        assert_eq!(o.project.meta.name, "Pump station");
        assert_eq!(o.project.meta.title_block.len(), 1);
        assert!(o.undo());
        assert_eq!(o.project.meta.name, "Test");
    }

    #[test]
    fn a_jumper_stays_on_its_terminals_when_one_moves_past_another() {
        let (_tmp, mut o) = project();
        let page = o.project.pages.iter().position(|p| p.id == "p0002").unwrap();
        let x1 = DeviceTag::parse("-X1").unwrap();
        let mut put = |at: f64| {
            let id = o.place(page, "wc.terminal", Point::new(at, 20.0)).unwrap();
            let s = o.project.pages[page].symbols.iter_mut().find(|s| s.id == id).unwrap();
            s.tag = Some(x1.clone());
            id
        };
        let (a, b, c) = (put(10.0), put(20.0), put(30.0));
        // Numbered by drawing order: a 1, b 2, c 3. Jumper a and b.
        o.add_jumper(&x1, &["1".into(), "2".into()]).unwrap();
        // Move a to the far right: now b 1, c 2, a 3. The jumper must still join a and b.
        o.checkpoint();
        o.project.pages[page].symbols.iter_mut().find(|s| s.id == a).unwrap().at = Point::new(40.0, 20.0);
        assert!(o.follow_terminals());
        assert_eq!(o.project.meta.jumpers[0].terminals, ["1", "3"]);
        let n = o.terminal_numbers();
        let num = |id: &str| n[&(o.project.pages[page].id.clone(), id.to_owned())].1.clone();
        assert_eq!((num(&a), num(&b), num(&c)), ("3".into(), "1".into(), "2".into()));
        // Undo puts drawing and jumper back together, with nothing left to follow.
        assert!(o.undo());
        assert_eq!(o.project.meta.jumpers[0].terminals, ["1", "2"]);
        assert!(!o.follow_terminals());
    }

    #[test]
    fn a_drag_followed_frame_by_frame_keeps_the_jumper() {
        let (_tmp, mut o) = project();
        let page = o.project.pages.iter().position(|p| p.id == "p0002").unwrap();
        let x1 = DeviceTag::parse("-X1").unwrap();
        let mut put = |at: f64| {
            let id = o.place(page, "wc.terminal", Point::new(at, 20.0)).unwrap();
            let s = o.project.pages[page].symbols.iter_mut().find(|s| s.id == id).unwrap();
            s.tag = Some(x1.clone());
            id
        };
        let (a, b, _c) = (put(10.0), put(20.0), put(30.0));
        o.add_jumper(&x1, &["1".into(), "2".into()]).unwrap();
        // One drag, one checkpoint, two frames: a passes b, then c.
        o.checkpoint();
        let mut to = |x: f64| {
            o.project.pages[page].symbols.iter_mut().find(|s| s.id == a).unwrap().at = Point::new(x, 20.0);
            o.follow_terminals();
        };
        to(25.0);
        to(40.0);
        let n = o.terminal_numbers();
        let num = |id: &str| n[&(o.project.pages[page].id.clone(), id.to_owned())].1.clone();
        let mut want = vec![num(&a), num(&b)];
        want.sort();
        assert_eq!(o.project.meta.jumpers[0].terminals, want);
    }

    #[test]
    fn a_terminal_turned_into_another_symbol_leaves_its_jumper() {
        let (_tmp, mut o) = project();
        let page = o.project.pages.iter().position(|p| p.id == "p0002").unwrap();
        let x1 = DeviceTag::parse("-X1").unwrap();
        let mut put = |at: f64| {
            let id = o.place(page, "wc.terminal", Point::new(at, 20.0)).unwrap();
            let s = o.project.pages[page].symbols.iter_mut().find(|s| s.id == id).unwrap();
            s.tag = Some(x1.clone());
            id
        };
        let (a, _b, _c, _d) = (put(10.0), put(20.0), put(30.0), put(40.0));
        o.add_jumper(&x1, &["1".into(), "2".into(), "3".into()]).unwrap();
        // a becomes a coil: b, c, d are now 1, 2, 3 and the jumper keeps b and c only.
        o.checkpoint();
        o.project.pages[page]
            .symbols
            .iter_mut()
            .find(|s| s.id == a)
            .unwrap()
            .symbol = "wc.coil".into();
        o.follow_terminals();
        assert_eq!(o.project.meta.jumpers[0].terminals, ["1", "2"]);
    }

    #[test]
    fn an_edit_after_a_move_keeps_the_followed_jumper() {
        let (_tmp, mut o) = project();
        let page = o.project.pages.iter().position(|p| p.id == "p0002").unwrap();
        let x1 = DeviceTag::parse("-X1").unwrap();
        let mut put = |at: f64| {
            let id = o.place(page, "wc.terminal", Point::new(at, 20.0)).unwrap();
            let s = o.project.pages[page].symbols.iter_mut().find(|s| s.id == id).unwrap();
            s.tag = Some(x1.clone());
            id
        };
        let (a, _b, _c) = (put(10.0), put(20.0), put(30.0));
        o.add_jumper(&x1, &["1".into(), "2".into()]).unwrap();
        // Move a past b and c, then make unrelated edits that copy the project or its meta
        // first: none of them may bring back the stale jumper.
        o.checkpoint();
        o.project.pages[page].symbols.iter_mut().find(|s| s.id == a).unwrap().at = Point::new(40.0, 20.0);
        let x2 = DeviceTag::parse("-X2").unwrap();
        assert!(!o.remove_from_jumper(&x2, "1"));
        assert_eq!(o.project.meta.jumpers[0].terminals, ["1", "3"]);
        // An unnumbered wire, so numbering really replaces the project with its copy.
        let k1 = o.place(page, "wc.coil", Point::new(20.0, 120.0)).unwrap();
        let k2 = o.place(page, "wc.coil", Point::new(60.0, 160.0)).unwrap();
        let pin = |item: &str, pin: &str| Endpoint::Pin {
            item: item.into(),
            pin: pin.into(),
        };
        o.add_wire(page, pin(&k1, "A2"), pin(&k2, "A1"), Vec::new());
        o.checkpoint();
        o.project.pages[page].symbols.iter_mut().find(|s| s.id == a).unwrap().at = Point::new(5.0, 20.0);
        assert!(o.number_wires(wc_core::numbering::Rule::Sequential).done.wires > 0);
        assert_eq!(o.project.meta.jumpers[0].terminals, ["1", "2"]);
        o.set_device_property(&x2, "note", "x");
        assert_eq!(o.project.meta.jumpers[0].terminals, ["1", "2"]);
    }

    #[test]
    fn jumper_edits_after_a_move_start_from_the_followed_jumper() {
        let (_tmp, mut o) = project();
        let page = o.project.pages.iter().position(|p| p.id == "p0002").unwrap();
        let x1 = DeviceTag::parse("-X1").unwrap();
        let mut put = |at: f64| {
            let id = o.place(page, "wc.terminal", Point::new(at, 20.0)).unwrap();
            let s = o.project.pages[page].symbols.iter_mut().find(|s| s.id == id).unwrap();
            s.tag = Some(x1.clone());
            id
        };
        let (a, _b, _c, _d) = (put(10.0), put(20.0), put(30.0), put(50.0));
        o.add_jumper(&x1, &["1".into(), "2".into()]).unwrap();
        // Move a past b and c (b 1, c 2, a 3, d 4), then jumper a (now 3) to d: one jumper of
        // a, b and d.
        o.checkpoint();
        o.project.pages[page].symbols.iter_mut().find(|s| s.id == a).unwrap().at = Point::new(40.0, 20.0);
        o.add_jumper(&x1, &["3".into(), "4".into()]).unwrap();
        assert_eq!(o.project.meta.jumpers.len(), 1);
        assert_eq!(o.project.meta.jumpers[0].terminals, ["1", "3", "4"]);
        // Move a back to the left (a 1, b 2, c 3, d 4), then take d out: a and b remain.
        o.checkpoint();
        o.project.pages[page].symbols.iter_mut().find(|s| s.id == a).unwrap().at = Point::new(5.0, 20.0);
        assert!(o.remove_from_jumper(&x1, "4"));
        assert_eq!(o.project.meta.jumpers[0].terminals, ["1", "2"]);
    }

    #[test]
    fn a_jumper_part_set_after_a_move_keeps_the_followed_jumper() {
        let (_tmp, mut o) = project();
        let page = o.project.pages.iter().position(|p| p.id == "p0002").unwrap();
        let x1 = DeviceTag::parse("-X1").unwrap();
        let mut put = |at: f64| {
            let id = o.place(page, "wc.terminal", Point::new(at, 20.0)).unwrap();
            let s = o.project.pages[page].symbols.iter_mut().find(|s| s.id == id).unwrap();
            s.tag = Some(x1.clone());
            id
        };
        let (a, _b, _c) = (put(10.0), put(20.0), put(30.0));
        o.add_jumper(&x1, &["1".into(), "2".into()]).unwrap();
        // Move a past c (b 1, c 2, a 3): the jumper of a and b is now 1-3.
        o.checkpoint();
        o.project.pages[page].symbols.iter_mut().find(|s| s.id == a).unwrap().at = Point::new(40.0, 20.0);
        o.set_jumper_part(&x1, "1", Some("example.ex-tb-2-5")).unwrap();
        assert_eq!(o.project.meta.jumpers.len(), 1);
        assert_eq!(o.project.meta.jumpers[0].terminals, ["1", "3"]);
        assert_eq!(o.project.meta.jumpers[0].part.as_deref(), Some("example.ex-tb-2-5"));
    }

    #[test]
    fn a_terminal_whose_page_becomes_an_overview_does_not_take_another_terminals_number() {
        let (_tmp, mut o) = project();
        let first = o.project.pages.iter().position(|p| p.id == "p0002").unwrap();
        o.project.pages.push(Page::new("p0003", "3", "More"));
        let second = o.project.pages.len() - 1;
        let x1 = DeviceTag::parse("-X1").unwrap();
        let mut put = |page: usize, at: f64| {
            let id = o.place(page, "wc.terminal", Point::new(at, 20.0)).unwrap();
            let s = o.project.pages[page].symbols.iter_mut().find(|s| s.id == id).unwrap();
            s.tag = Some(x1.clone());
            id
        };
        // a on the first page, b and c on the second: a 1, b 2, c 3. Jumper a and c.
        let (_a, _b, _c) = (put(first, 10.0), put(second, 10.0), put(second, 20.0));
        o.add_jumper(&x1, &["1".into(), "3".into()]).unwrap();
        // The first page becomes an overview: b 1, c 2. a is no terminal of the plan any more,
        // and must not pass its old number 1 on to b.
        o.checkpoint();
        o.project.pages[first].overview = true;
        o.follow_terminals();
        assert!(
            o.project.meta.jumpers.iter().all(|j| j.terminals != ["1", "2"]),
            "{:?}",
            o.project.meta.jumpers
        );
        assert!(o.project.meta.jumpers.is_empty(), "only c is left of the jumper");
    }

    #[test]
    fn redo_brings_back_a_followed_move_and_an_overview_page_keeps_its_jumpers() {
        let (_tmp, mut o) = project();
        let page = o.project.pages.iter().position(|p| p.id == "p0002").unwrap();
        let x1 = DeviceTag::parse("-X1").unwrap();
        let mut put = |at: f64| {
            let id = o.place(page, "wc.terminal", Point::new(at, 20.0)).unwrap();
            o.project.pages[page]
                .symbols
                .iter_mut()
                .find(|s| s.id == id)
                .unwrap()
                .tag = Some(x1.clone());
            id
        };
        let (a, _b, _c) = (put(10.0), put(20.0), put(30.0));
        o.add_jumper(&x1, &["1".into(), "2".into()]).unwrap();
        o.checkpoint();
        o.project.pages[page].symbols.iter_mut().find(|s| s.id == a).unwrap().at = Point::new(40.0, 20.0);
        // Undo right away (no checkpoint in between), then redo: the move comes back followed.
        assert!(o.undo());
        assert_eq!(o.project.meta.jumpers[0].terminals, ["1", "2"]);
        assert!(o.redo());
        assert_eq!(o.project.meta.jumpers[0].terminals, ["1", "3"]);
        // Marking the page as an overview does not delete the jumper.
        o.checkpoint();
        o.project.pages[page].overview = true;
        o.follow_terminals();
        assert_eq!(o.project.meta.jumpers[0].terminals, ["1", "3"]);
    }

    #[test]
    fn project_props_set_the_locations_in_one_undo_step() {
        let (_tmp, mut o) = project();
        let name = o.project.meta.name.clone();
        let mark = o.undo_mark();
        let rows = |r: &[(&str, &str)]| {
            r.iter()
                .map(|(a, b)| (a.to_string(), b.to_string()))
                .collect::<Vec<_>>()
        };
        o.set_project_props(&name, &[], Some(&rows(&[("+L2", "Room"), ("", ""), ("L1", "Cabinet")])))
            .unwrap();
        assert_eq!(o.undo_mark(), mark + 1);
        let paths: Vec<&str> = o.project.meta.locations.iter().map(|l| l.path.as_str()).collect();
        assert_eq!(paths, ["L1", "L2"]);
        assert!(o.set_project_props(&name, &[], Some(&rows(&[("+L1-K1", "")]))).is_err());
        assert!(
            o.set_project_props(&name, &[], Some(&rows(&[("L1", "a"), ("+L1", "b")])))
                .is_err(),
            "twice"
        );
        assert!(
            o.set_project_props(&name, &[], Some(&rows(&[("", "Cabinet")])))
                .is_err(),
            "no path"
        );
        assert_eq!(o.project.meta.locations.len(), 2, "a bad row changes nothing");
        o.set_project_props(&name, &[], None).unwrap();
        assert_eq!(o.project.meta.locations.len(), 2, "None keeps them");
        o.set_project_props(&name, &[], Some(&[])).unwrap();
        assert!(o.project.meta.locations.is_empty());
        assert!(o.undo());
        assert_eq!(o.project.meta.locations.len(), 2);
    }

    #[test]
    fn undo_and_redo_of_a_new_library_source_update_the_library_list() {
        let (_tmp, mut o) = project();
        let root = o.dir.join("library");
        let sym = wc_core::Symbol {
            id: "test.imported".into(),
            name: "Imported".into(),
            category: String::new(),
            letter_code: String::new(),
            role: wc_core::symbol::SymbolRole::Standalone,
            graphics: vec![],
            pins: vec![],
            tag_at: Default::default(),
            description: String::new(),
        };
        wc_library::write_symbol(&root, &sym).unwrap();
        let listed = |o: &Open| o.library.iter().any(|(id, _)| id == "test.imported");
        o.refresh_library();
        assert!(!listed(&o), "the folder is not a source yet");

        // As Library -> Import symbols does the first time it writes into the project's folder.
        o.checkpoint();
        o.project.meta.libraries.insert(
            0,
            LibrarySource {
                name: "project".into(),
                kind: "folder".into(),
                location: "library".into(),
                reference: None,
            },
        );
        o.refresh_library();
        assert!(listed(&o));
        assert!(o.undo());
        assert!(!listed(&o), "undo removed the source, so its symbols go");
        assert!(o.redo());
        assert!(listed(&o), "redo brings them back");

        // A part assigned after the source: undo both steps, then redo both. The rebuild on the
        // first redo drops the part (no device uses it yet); the second redo loads it again.
        let part = Part {
            id: "acme.relay".into(),
            manufacturer: "Acme".into(),
            ..Default::default()
        };
        store::write_vendored_part(&o.dir, &part).unwrap();
        o.checkpoint();
        o.project.devices.insert(
            DeviceTag::parse("-K1").unwrap(),
            wc_core::Device {
                part: Some(part.id.clone()),
                ..Default::default()
            },
        );
        o.parts.insert(part.id.clone(), part.clone());
        assert!(o.undo() && o.undo());
        assert!(!o.parts.contains_key("acme.relay"));
        assert!(o.redo() && o.redo());
        assert_eq!(
            o.parts.get("acme.relay"),
            Some(&part),
            "the device's part is loaded again"
        );
        assert_eq!(o.take_step_warning(), None);

        // A part file that is gone: the step is done, and the warning names the part and why.
        assert!(o.undo());
        std::fs::remove_file(store::vendor_path(&o.dir, ItemKind::Part, "acme.relay").unwrap()).unwrap();
        o.parts.remove("acme.relay");
        assert!(o.redo());
        let warning = o.take_step_warning().unwrap_or_default();
        assert!(warning.contains("acme.relay"), "{warning}");
    }
}
