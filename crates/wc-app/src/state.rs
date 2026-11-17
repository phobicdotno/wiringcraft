//! The open project and everything the user is doing with it, independent of drawing.

use eframe::egui;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use wc_core::geom::{Point, Rotation};
use wc_core::lock::ItemKind;
use wc_core::project::PageSize;
use wc_core::route::{self, Axis, Route};
use wc_core::{DeviceTag, Endpoint, Page, Part, PlacedSymbol, Project, Symbol, Wire, store};

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
    Comment,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Selection {
    Symbol(String),
    Wire(String),
    /// A free drawing on the page (line, rectangle, circle, text), by its index in `graphics`.
    Graphic(usize),
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
        };
        me.refresh_library();
        Ok(me)
    }

    /// The texture for a page's underlay picture, loaded on first use. Pictures larger than the
    /// graphics card takes are scaled down to fit.
    pub fn underlay_texture(&mut self, ctx: &egui::Context, u: &wc_core::Underlay) -> Option<egui::TextureHandle> {
        if let Some(t) = self.underlays.get(&u.file) {
            return t.clone();
        }
        let max_side = ctx.input(|i| i.max_texture_side).max(1) as u32;
        let tex = u
            .file_is_safe()
            .then(|| std::fs::read(self.dir.join(&u.file)).ok())
            .flatten()
            .and_then(|bytes| image::load_from_memory(&bytes).ok())
            .map(|img| {
                if img.width() > max_side || img.height() > max_side {
                    img.resize(max_side, max_side, image::imageops::FilterType::Triangle)
                } else {
                    img
                }
            })
            .map(|img| {
                let rgba = img.to_rgba8();
                let size = [rgba.width() as usize, rgba.height() as usize];
                let pixels = egui::ColorImage::from_rgba_unmultiplied(size, rgba.as_raw());
                ctx.load_texture(format!("underlay:{}", u.file), pixels, egui::TextureOptions::LINEAR)
            });
        self.underlays.insert(u.file.clone(), tex.clone());
        tex
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
            .devices
            .values()
            .filter_map(|d| d.part.as_ref())
            .filter_map(|id| store::read_vendored_part(&self.dir, id).ok().map(|p| (id.clone(), p)))
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
        if let Some(id) = part
            && !self.parts.contains_key(id)
        {
            let p = match store::read_vendored_part(&self.dir, id) {
                Ok(p) => p,
                Err(_) => {
                    let lib = wc_library::Library::open(&self.dir, &self.project).map_err(|e| e.to_string())?;
                    wc_library::take(&self.dir, &mut self.project, &lib, ItemKind::Part, id)
                        .map_err(|e| e.to_string())?;
                    store::read_vendored_part(&self.dir, id).map_err(|e| e.to_string())?
                }
            };
            self.parts.insert(id.to_owned(), p);
        }
        self.checkpoint();
        self.project.devices.entry(tag.clone()).or_default().part = part.map(str::to_owned);
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

    /// Take a snapshot before a change, so it can be undone.
    pub fn checkpoint(&mut self) {
        self.undo.push(self.project.clone());
        if self.undo.len() > 200 {
            self.undo.remove(0);
            self.undo_dropped += 1;
        }
        self.redo.clear();
        self.dirty = true;
        self.history += 1;
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
        match self.undo.pop() {
            Some(prev) => {
                self.redo.push(std::mem::replace(&mut self.project, prev));
                self.dirty = true;
                self.history += 1;
                true
            }
            None => false,
        }
    }

    pub fn redo(&mut self) -> bool {
        match self.redo.pop() {
            Some(next) => {
                self.undo.push(std::mem::replace(&mut self.project, next));
                self.dirty = true;
                self.history += 1;
                true
            }
            None => false,
        }
    }

    pub fn save(&mut self) -> Result<usize, String> {
        let n = store::save(&self.dir, &self.project).map_err(|e| e.to_string())?.len();
        self.dirty = false;
        Ok(n)
    }

    /// Make sure a library symbol is vendored and pinned before it is placed.
    pub fn ensure_symbol(&mut self, id: &str) -> Result<(), String> {
        if self.symbols.contains_key(id) {
            return Ok(());
        }
        let lib = wc_library::Library::open(&self.dir, &self.project).map_err(|e| e.to_string())?;
        wc_library::take(&self.dir, &mut self.project, &lib, ItemKind::Symbol, id).map_err(|e| e.to_string())?;
        let s = store::read_vendored_symbol(&self.dir, id).map_err(|e| e.to_string())?;
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

    /// Change a page's number, title and size. A number another page already uses is refused.
    pub fn set_page_props(
        &mut self,
        page: usize,
        number: &str,
        title: &str,
        size: PageSize,
        overview: bool,
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
        if current.number == number && current.title == title && current.size == size && current.overview == overview {
            return Ok(page);
        }
        self.checkpoint();
        if let Some(p) = self.project.pages.get_mut(page) {
            p.number = number.to_owned();
            p.title = title.trim().to_owned();
            p.size = size;
            p.overview = overview;
        }
        self.project.sort_pages();
        Ok(self.index_of(&id))
    }

    /// Delete a page. The last page of a project cannot be deleted. Returns the page to show next.
    pub fn delete_page(&mut self, page: usize) -> Result<usize, String> {
        if self.project.pages.len() <= 1 {
            return Err("A project needs at least one page.".into());
        }
        if page >= self.project.pages.len() {
            return Err("No such page.".into());
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
        let m = wc_core::macros::read_macro(path).map_err(|e| e.to_string())?;
        let clip = crate::edit::Clipboard::from(m);
        let at = clip.anchor;
        Ok(self.paste(page, &clip, at))
    }

    /// Number the wires by `rule`, keeping numbers already given. One undo step; nothing is
    /// recorded when no number changes.
    pub fn number_wires(&mut self, rule: wc_core::numbering::Rule) -> wc_core::numbering::Numbered {
        let mut project = self.project.clone();
        let symbols = &self.symbols;
        let done = wc_core::numbering::number_wires(
            &mut project,
            rule,
            false,
            |id| {
                symbols
                    .get(id)
                    .is_some_and(|s| s.role == wc_core::symbol::SymbolRole::Interruption)
            },
            |page, end| match end {
                wc_core::Endpoint::Point { at } => Some(*at),
                wc_core::Endpoint::Pin { item, pin } => {
                    let placed = page.symbol(item)?;
                    wc_pdf::pin_position(placed, symbols.get(&placed.symbol)?, pin)
                }
            },
        );
        if done.wires > 0 {
            self.checkpoint();
            self.project = project;
        }
        done
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

    /// Rename the project and set its title-block fields (empty values are dropped).
    pub fn set_project_props(&mut self, name: &str, fields: &[(String, String)]) -> Result<(), String> {
        let name = name.trim();
        if name.is_empty() {
            return Err("The project name cannot be empty.".into());
        }
        let block = title_block_of(fields);
        if self.project.meta.name == name && self.project.meta.title_block == block {
            return Ok(());
        }
        self.checkpoint();
        self.project.meta.name = name.to_owned();
        self.project.meta.title_block = block;
        Ok(())
    }

    /// The cover page's index. A project without one gets one: an empty first page titled
    /// "Cover" (as new projects started) is marked as the cover, otherwise a new cover page
    /// numbered `0` goes in front of the others. Creating or marking it is one undo step. Fails,
    /// changing nothing, when a page already uses a number that sorts as low as `0`.
    pub fn cover_page(&mut self) -> Result<usize, String> {
        if let Some(i) = self.project.cover_page() {
            return Ok(i);
        }
        if let Some(first) = self.project.pages.first()
            && first.title.trim().eq_ignore_ascii_case("cover")
            && first.symbols.is_empty()
            && first.wires.is_empty()
            && first.graphics.is_empty()
            && first.underlay.is_none()
        {
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

    /// A properties form opens: its live edits will make a fresh undo step. Returns the undo
    /// mark to pass to [`Open::revert_live`].
    pub fn start_live_form(&mut self) -> usize {
        self.form_step = None;
        self.undo_mark()
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
        // Keep adding to the form's own undo step only while nothing else has touched the
        // history since; after any other change (a page added, another properties edit, an
        // undo) the form starts a new step.
        if self.form_step == Some(self.history) {
            self.dirty = true;
        } else {
            self.checkpoint();
            self.form_step = Some(self.history);
        }
        if name_changes {
            self.project.meta.name = name.to_owned();
        }
        self.project.meta.title_block = block;
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

    fn fields(pairs: &[(&str, &str)]) -> Vec<(String, String)> {
        pairs.iter().map(|(k, v)| ((*k).to_owned(), (*v).to_owned())).collect()
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
        o.set_project_props("Test", &fields(&[("customer", "X")])).unwrap();
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
    fn pages_can_be_renamed_moved_duplicated_and_deleted() {
        let (_tmp, mut o) = project();
        assert_eq!(o.project.pages.len(), 2);
        // Renumbering resorts; a taken number is refused.
        assert!(o.set_page_props(0, "2", "Cover", PageSize::A3, false).is_err());
        let i = o.set_page_props(0, "10", "Cover sheet", PageSize::A4, true).unwrap();
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
    fn project_props_drop_empty_title_block_fields() {
        let (_tmp, mut o) = project();
        assert!(o.set_project_props(" ", &[]).is_err());
        o.set_project_props(
            "Pump station",
            &[("customer".into(), "Acme".into()), ("revision".into(), " ".into())],
        )
        .unwrap();
        assert_eq!(o.project.meta.name, "Pump station");
        assert_eq!(o.project.meta.title_block.len(), 1);
        assert!(o.undo());
        assert_eq!(o.project.meta.name, "Test");
    }
}
