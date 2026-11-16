//! The open project and everything the user is doing with it, independent of drawing.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use wc_core::geom::{Point, Rotation};
use wc_core::lock::ItemKind;
use wc_core::project::PageSize;
use wc_core::{DeviceTag, Endpoint, Page, PlacedSymbol, Project, Symbol, Wire, store};

/// What a click on the canvas does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tool {
    Select,
    Place,
    Wire,
    Text,
    Line,
    Rect,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Selection {
    Symbol(String),
    Wire(String),
    /// A free drawing on the page (line, rectangle, circle, text), by its index in `graphics`.
    Graphic(usize),
}

pub struct Open {
    pub dir: PathBuf,
    pub project: Project,
    /// The project's vendored symbols.
    pub symbols: BTreeMap<String, Symbol>,
    /// Ids of every symbol in the project's library sources (for the picker).
    pub library: Vec<(String, String)>,
    pub dirty: bool,
    undo: Vec<Project>,
    redo: Vec<Project>,
    pub branch: String,
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
            dirty: false,
            undo: Vec::new(),
            redo: Vec::new(),
            branch: wc_vcs::Git::new(dir).current_branch().unwrap_or_default(),
        };
        me.refresh_library();
        Ok(me)
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
    }

    /// Take a snapshot before a change, so it can be undone.
    pub fn checkpoint(&mut self) {
        self.undo.push(self.project.clone());
        if self.undo.len() > 200 {
            self.undo.remove(0);
        }
        self.redo.clear();
        self.dirty = true;
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
            });
        }
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
    pub fn set_page_props(&mut self, page: usize, number: &str, title: &str, size: PageSize) -> Result<usize, String> {
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
        if current.number == number && current.title == title && current.size == size {
            return Ok(page);
        }
        self.checkpoint();
        if let Some(p) = self.project.pages.get_mut(page) {
            p.number = number.to_owned();
            p.title = title.trim().to_owned();
            p.size = size;
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
    pub fn duplicate_page(&mut self, page: usize) -> Option<usize> {
        let mut copy = self.project.pages.get(page)?.clone();
        self.checkpoint();
        copy.id = self.free_page_id();
        copy.number = self.next_page_number();
        copy.title = format!("{} (copy)", copy.title);
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
        let block: BTreeMap<String, String> = fields
            .iter()
            .map(|(k, v)| (k.trim().to_owned(), v.trim().to_owned()))
            .filter(|(k, v)| !k.is_empty() && !v.is_empty())
            .collect();
        if self.project.meta.name == name && self.project.meta.title_block == block {
            return Ok(());
        }
        self.checkpoint();
        self.project.meta.name = name.to_owned();
        self.project.meta.title_block = block;
        Ok(())
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
        assert!(o.set_page_props(0, "2", "Cover", PageSize::A3).is_err());
        let i = o.set_page_props(0, "10", "Cover sheet", PageSize::A4).unwrap();
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
