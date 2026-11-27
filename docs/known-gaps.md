# Known gaps and where they are tracked

Audit for issue #49 (November 2026): every known limitation, open branch and code marker, and
the todo that covers it, or why it has none. New gaps go straight to the todo list (see
[CONTRIBUTING.md](../CONTRIBUTING.md)); this page is a snapshot, not the list itself.

## How the audit was done

- **Code markers:** searched `crates/` and `tools/` for `TODO`, `FIXME` and `XXX`. None are left
  (the only hit is the `\U+XXXX` escape described in `crates/wc-pdf/src/vector.rs`).
- **Limits in code:** searched for "not supported", "not yet", "left out" and "skipped" in
  `crates/`. Most hits are deliberate, documented behaviour (hostile input is skipped with a
  warning); the real feature gaps are listed below.
- **README and docs:** read `README.md` and `docs/manufacturer-data.md`.
- **Branches:** every remote branch not merged into `main` was checked against its pull request
  (last checked 26 November 2026).

## Branches

There are 235 remote branches not merged into `main`. 231 of them belong to a merged pull
request (squash-merged, so git does not see them as merged); they hold no lost work and can be
deleted. The other four:

| Branch | Pull request |
|---|---|
| `app/file-pickers` | #344 (open, #338) |
| `app/imports` | #349 (open, #338) |
| `import/dwg-2013-2018` | #299 (open, #292) |
| `claude/issue-418-pdf-runlength` | #432 (closed without merging; the same work went in with #427, fixing #418), can be deleted |

## Import

| Gap | Where it shows | Todo |
|---|---|---|
| EDZ exchange files: only the multi-line representation, variant A, of an EPLAN macro (`.ema`) becomes a symbol; panel layout, overview, single-line and 3D representations and other variants are counted, not imported. Symbols from EPLAN's own libraries (coils, contacts, lamps) are drawn as the WiringCraft symbol of the same kind, not with EPLAN's drawing, and connection points placed from those libraries keep their pin but not their drawing. Text alignment numbers are read as left, centre, right in rows of three (inferred), and only the horizontal part is used. Connection points that only other multi-line variants (or, without those, the overview) hold are added to the symbol as pins in a column to the right of its drawing, without their drawing (#511); symbols from EPLAN's own libraries get one pin per connection point designation (a terminal with 4 designations has 4, in top and bottom pairs a grid step apart, since #512) but their pin positions follow WiringCraft's drawing of the same kind, not EPLAN's; category 502 is drawn as fuses, one per pair of points (inferred from maker macros); device tag, connection designation and description text placements and terminal descriptions (A186) are not mapped, and the tag position is a guess; a macro whose maker cannot be told (no part names it and the archive holds parts of several makers, or none) keeps the id `eplan.<file name>`, with a warning, and can still be overwritten by another archive's macro of the same name | `crates/wc-import/src/edz/macros.rs` | #513 |
| SolidWorks Electrical: symbol and page graphics come from the archive's DWG files (`Block/<name>.dwg`, `Projects/<n>/Drawings/<page id>.ewg`); the file layout is checked on synthetic archives only, texts of a symbol scaled differently along x and y keep their size from the mean scale (its circles and arcs are drawn as ellipses since #307), and a bare database import still draws boxes | `crates/wc-import/src/swe/drawings.rs` | Known limit: no real archive to check against |
| SolidWorks Electrical: page revisions, and the project's revision and checked by from the book's revision rows (since #170), are read from `tew_revision` by column name (the layout is not published), checked on synthetic databases only; with several books the first book's row wins, with a warning when they differ | `crates/wc-import/src/swe.rs` | Known limit: no real database to check against |
| SolidWorks Electrical libraries (`import swe-library`): catalog parts bring their terminals as a text property, not as pins of a symbol; library blocks are drawn from their DWG files (#59), and blocks without a readable DWG file in the archive stay box symbols | `crates/wc-import/src/swe/library.rs` | Known limit: a catalog part names no symbol, so its terminals stay a property |
| PDF trace: pictures in other sample layouts than 1-bit gray, 8- or 16-bit gray and RGB and 8-bit CMYK, are refused ("not supported yet"). LZW, CMYK and 16-bit are read since #171; CCITT fax (G3 and G4), JBIG2 and JPEG 2000 since #271, RunLength since #418, checked on small generated samples only (a JPEG 2000 alpha channel is dropped; JBIG2 symbol dictionaries shared through `JBIG2Globals` are tested since #518, and a picture that refers to a segment neither it nor its `JBIG2Globals` holds in full is refused with a message instead of coming out blank since #523; segments after the end of the file segment are not read). The JBIG2 decoder stops once its bitmaps (regions, pattern and symbol dictionaries, page) go over a 256 MiB limit, half for the shared `JBIG2Globals` segments and half for the page, less what symbol dictionaries keep outside it, their decoder contexts and export lists (counted from their headers); text-like scans with symbol dictionaries are read since #423, using an unreleased `hayro-jbig2` commit until a release has the memory limit | `crates/wc-import/src/pdf.rs`, `crates/wc-import/src/pdf_codecs.rs` | #517 (crates.io release) |
| SolidWorks Electrical cabinet layouts: items are sorted by block name (rail, duct, plate) since the database has no item kind; a footprint without a box size takes the box round its block's DWG drawing, else its catalog part's width and height, else 18 x 90 mm; upright rails are left out (WiringCraft rails run left to right) | `crates/wc-import/src/swe/cabinet.rs` | Known limit: the database has no item kind, so sorting by name stays |
| SVG symbols: the import gives no warnings for what it leaves out. `<use>`, `<image>` and other elements are skipped; `style` is read only for fill, fill opacity 0, fill rule, font and visibility; a filled path whose sub-paths cross each other is filled as if they were nested or apart, not where they overlap; an ellipse comes in as a circle; a `<tspan>` moved only with `dx`/`dy` stays on its text's line | `crates/wc-import/src/svg.rs` | #952, #953, #900, #1134 |
| KiCad symbols: old `.lib` libraries are not read (KiCad 6 `.kicad_sym` only); alternate body styles and hidden pins are left out, with a warning; fields shown on the symbol, such as Value, come in as texts, and hidden ones are left out; units after the 64th are not read | `crates/wc-import/src/kicad.rs` | #957; Known limit for `.lib` (the format before KiCad 6) and the 64 units (real parts have a few) |
| QElectroTech elements: the letter code is taken from the element's `label` information only when it reads as a reference (`-K1`, `PG2`), else it is left empty; a text keeps its centre or right alignment only when the file anchors its place to the alignment (`anchor="alignment"`), else it comes in left-aligned at its left end, and the lines of a multi-line text are not lined up | `crates/wc-import/src/qet.rs` | Known limit: QElectroTech's label is free text, and without the anchor the file gives no point other than the left end |
| Binary DXF | `crates/wc-import/src/dxf.rs` | Not planned: the error tells the user to save as ASCII DXF, which every CAD program can do. |
| DWG | `crates/wc-import/src/dwg.rs` | Read for AutoCAD 2004 to 2018 drawings only (`AC1018` to `AC1032`; AutoCAD releases after 2018 still save in the 2018 format, so their drawings are read too; 2007 checked against one real file, #215; 2013 and 2018 against 143 real files, #292; the class section is read as the specification gives it: 2007 without the second size field, 2010 and 2013 with it from maintenance release 4, 2018 always with it; the class layout of 2007, 2013 and 2018 has only been checked with drawings the tests write, and a class section that cannot be read gives a warning and the drawing is still read); 2004 texts in a code page the reader has no table for (the multi-byte Chinese, Japanese and Korean pages, the DOS pages, ISO 8859 pages other than 8859-1) are read with the legacy decoder, UTF-8 when the bytes are and else as Windows-1252, with a warning; a DWG page import (`import page-dwg`, and the app's File > Import > DWG page, which runs the same code since #600) uses this reader and turns to an installed DWG to DXF converter only for a file it refuses (#600). Multi-line attributes of 2018 drawings are counted and skipped: #300. |
| STEP (3D) | `docs/manufacturer-data.md` | Later: 3D belongs to the one-way export to SolidCraft (phase 5). |

## Editing and app

| Gap | Todo |
|---|---|
| Assistant: `get_report` cannot read the plc-cards and networks reports | #537 |
| Assistant: `add_graphic` and `update_graphic` cannot make dimensions or filled polylines | #539 |
| Assistant: the `run_command` tool description leaves out half the command line | #540 |
| Starter library: symbols and example parts still missing (timer relays, motor protection, process switches, sensors, drives, special terminals, lamp and button colours) | #531 to #536, #538 |

Fixed since the first audit: wire route styles (#52), Delete in the wire right-click menu (#88),
row letters and grid coordinates (#54), slow assistant tools off the window thread (#67),
editing a page's own title-block fields in the app, CLI and assistant (#169), and File > Import
off the window thread with its progress shown (#356).

## Shared sessions

Shared sessions (#43) are done: the core (#79), the network transport (#74), discovery (#75) and
the app's Share session and Join session (#76) are in. In a session the app sends every change to the open page as one page
edit and project-level changes (devices, comments, panels, pages, project properties) as project
edits. Tools that change pages nobody else has open (wire numbering, a tag renamed on every page)
send those pages together as one change (#77); wire numbering leaves pages other people hold as
they are and says which. Undo takes back your own latest change, and "Undo this change" in the
session's change list takes back any change whose parts were not changed again since (#78).
Updating a symbol or part from the library during a session sends the new version to everyone
as one change, refused when someone else updated it first or when wires in the session end on a
pin the new version drops; undoing it puts the old version back in every copy (#197). An undo
is refused while a wire ends on a pin the version it brings back does not have, for example a
pin the update added that has been wired since: move or remove that wire, then undo again. A
joining copy takes a symbol the session did not send from its own libraries only in the version
the session pinned; otherwise it reports the symbol as missing. A page, device or jumper that uses
a symbol or part the session does not have yet waits in the window until the session takes the
item, however long that takes, and the session window says what it waits for (#198). Other changes
go out and others' changes keep coming in meanwhile. When the session changes that same page,
device or the project properties before it takes the item, the waiting change is dropped and a note
says so; a change of the page's number alone keeps it. Undo while a change waits takes that change
back here only. For the first few seconds after an item is sent, while the session answers,
everything waits, as before. The host refuses a page that places a symbol the project does not lock
(#286); placements a page already had stay editable. It also refuses a change or an undo that
gives a page a number another page has, as when two people renumber at once (#748); pages that
already shared a number stay editable.

Host and guests must run builds with the same session protocol version
(`wc_share::PROTOCOL_VERSION`, raised whenever a message changes). A guest on another version is
turned away when it asks to join (#201). A guest from this build on shows a message naming both
versions in Join session, and marks announced sessions on another version; a guest from before
versions were sent (counted as version 0) only shows that the host declined.

## Planning and integrations

| Gap | Todo |
|---|---|
| Full EPLAN / SolidWorks Electrical feature list with triage | Done (#55): [feature-matrix.md](feature-matrix.md) |
