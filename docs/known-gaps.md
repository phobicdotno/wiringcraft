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
- **Branches:** every remote branch not merged into `main` was checked against its pull request.

## Branches

All unmerged remote branches either belong to a merged pull request (squash-merged, so git does
not see them as merged) or to an open one. None hold lost work.

| Branch | Pull request |
|---|---|
| `app/egui-shell`, `app/pages`, `app/editing` | #3, #6, #7 (merged) |
| `import/aml-pdf-dwg`, `import/eplan`, `import/eplan-drawn-pages` | #2, #5, #16 (merged) |
| `phase1/library-import-pdf`, `reports/bom-wires-terminals`, `check`, `library/iec-basics` | #1, #4, #13, #14 (merged) |
| `todo/claim-permissions`, `build/installers` | #58, #72 (merged) |
| `claude/issue-43-share-session-core`, `-46-manufacturer-data`, `-48-rolling-log`, `-51-auto-wiring` | #79, #80, #81, #85 (merged) |
| `app/browse-folders` | #82 (open) |
| `claude/issue-52-wire-route-style` | #87 (open, fixes #52) |
| `import/swe-locations` | #89 (open, #69) |
| `frame/row-letters` | #90 (open, #54) |

These branches can be deleted once their pull requests are merged.

## Import

| Gap | Where it shows | Todo |
|---|---|---|
| EDZ exchange files: only the multi-line representation, variant A, of an EPLAN macro (`.ema`) becomes a symbol; panel layout, overview, single-line and 3D representations and other variants are counted, not imported. Symbols from EPLAN's own libraries (coils, contacts, lamps) are drawn as the WiringCraft symbol of the same kind, not with EPLAN's drawing, and connection points placed from those libraries keep their pin but not their drawing. Text alignment numbers are read as left, centre, right in rows of three (inferred), and only the horizontal part is used. Connection points that only other multi-line variants (or, without those, the overview) hold are added to the symbol as pins in a column to the right of its drawing, without their drawing (#511); pins of symbols from EPLAN's own libraries do not match EPLAN's positions or counts (category 502 falls back to a box, a terminal with 4 designations gets 2 pins); device tag, connection designation and description text placements and terminal descriptions (A186) are not mapped, and the tag position is a guess; a macro whose maker cannot be told (no part names it and the archive holds parts of several makers, or none) keeps the id `eplan.<file name>`, with a warning, and can still be overwritten by another archive's macro of the same name | `crates/wc-import/src/edz/macros.rs` | #512, #513 |
| SolidWorks Electrical: symbol and page graphics come from the archive's DWG files (`Block/<name>.dwg`, `Projects/<n>/Drawings/<page id>.ewg`); the file layout is checked on synthetic archives only, texts of a symbol scaled differently along x and y keep their size from the mean scale (its circles and arcs are drawn as ellipses), and a bare database import still draws boxes | `crates/wc-import/src/swe/drawings.rs` | #307 |
| SolidWorks Electrical: page revisions are read from `tew_revision` by column name (the layout is not published), checked on synthetic databases only; the project's own revision and checked-by are not read | `crates/wc-import/src/swe.rs` | #170 |
| SolidWorks Electrical libraries (`import swe-library`): catalog parts bring their terminals as a text property, not as pins of a symbol; library blocks without a readable DWG file in the archive stay box symbols | `crates/wc-import/src/swe/library.rs` | #59 |
| PDF trace: pictures in other sample layouts than 1-bit gray, 8- or 16-bit gray and RGB and 8-bit CMYK, are refused ("not supported yet"). LZW, CMYK and 16-bit are read since #171; CCITT fax (G3 and G4), JBIG2 and JPEG 2000 since #271, RunLength since #418, checked on small generated samples only (JBIG2 shared segments from `JBIG2Globals` are untested, and a JPEG 2000 alpha channel is dropped). The JBIG2 decoder stops once its bitmaps (regions, pattern and symbol dictionaries, page) go over a 256 MiB limit, half for the shared `JBIG2Globals` segments and half for the page, less what symbol dictionaries keep outside it, their decoder contexts and export lists (counted from their headers); text-like scans with symbol dictionaries are read since #423, using an unreleased `hayro-jbig2` commit until a release has the memory limit | `crates/wc-import/src/pdf.rs`, `crates/wc-import/src/pdf_codecs.rs` | #517 (crates.io release), #518 (JBIG2Globals test) |
| SolidWorks Electrical cabinet layouts: items are sorted by block name (rail, duct, plate) since the database has no item kind; a footprint without a box size takes the box round its block's DWG drawing, else its catalog part's width and height, else 18 x 90 mm; upright rails are left out (WiringCraft rails run left to right) | `crates/wc-import/src/swe/cabinet.rs` | Known limit: the database has no item kind, so sorting by name stays |
| Binary DXF | `crates/wc-import/src/dxf.rs` | Not planned: the error tells the user to save as ASCII DXF, which every CAD program can do. |
| DWG | `crates/wc-import/src/dwg.rs` | Read for AutoCAD 2004, 2007 and 2010 drawings only (2007 checked against one real file, #215; its class section is read as the specification gives it, like 2010 without the second size field, and has only been checked with drawings the tests write); 2004 texts in a code page the reader has no table for (the multi-byte Chinese, Japanese and Korean pages, the DOS pages, ISO 8859 pages other than 8859-1) are read with the legacy decoder, UTF-8 when the bytes are and else byte for byte as Latin-1 (with the euro sign and the two dashes of Windows-1252), with a warning; a DWG page import (`import page-dwg`) still goes through an installed DWG to DXF converter. |
| STEP (3D) | `docs/manufacturer-data.md` | Later: 3D belongs to the one-way export to SolidCraft (phase 5). |

## Editing and app

| Gap | Todo |
|---|---|
| A page's own title-block fields (revision, date, checked by) only come from an import; the app, CLI and assistant cannot edit them | #169 |
| Wire route styles | #52 (pull request #87) |
| Delete in the wire right-click menu | #88 |
| Row letters and grid coordinates | #54 (pull request #90) |
| Assistant runs slow tools on the window thread | #67 |

## Shared sessions

The core (#79), the network transport (#74), discovery (#75) and the app's Share session and
Join session (#76) are in. In a session the app sends every change to the open page as one page
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
(#286); placements a page already had stay editable.

Host and guests must run builds with the same session protocol version
(`wc_share::PROTOCOL_VERSION`, raised whenever a message changes). A guest on another version is
turned away when it asks to join (#201). A guest from this build on shows a message naming both
versions in Join session, and marks announced sessions on another version; a guest from before
versions were sent (counted as version 0) only shows that the host declined.

## Planning and integrations

| Gap | Todo |
|---|---|
| Full EPLAN / SolidWorks Electrical feature list with triage | #55 ([feature-matrix.md](feature-matrix.md)) |
