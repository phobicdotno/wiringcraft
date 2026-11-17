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
| EDZ exchange files: parts are read, EPLAN macros (`.ema`) only counted | `crates/wc-import/src/edz.rs` | #144 |
| SolidWorks Electrical: symbol and page graphics from DWG | `crates/wc-import/src/swe.rs` | #59 |
| SolidWorks Electrical: page revisions are read from `tew_revision` by column name (the layout is not published), checked on synthetic databases only; the project's own revision and checked-by are not read | `crates/wc-import/src/swe.rs` | #170 |
| SolidWorks Electrical libraries (`import swe-library`): catalog parts bring their terminals as a text property, not as pins of a symbol; library blocks are box symbols until the DWG reader draws them | `crates/wc-import/src/swe/library.rs` | #59 |
| SolidWorks Electrical: terminal jumpers made in the terminal strip editor are counted in the import report, not imported (WiringCraft has no jumpers yet) | `crates/wc-import/src/swe.rs` | #108 |
| PDF trace: pictures compressed other than Flate or DCT, or not 1-bit gray, 8-bit gray or 8-bit RGB, are refused ("not supported yet") | `crates/wc-import/src/pdf.rs` | #171 |
| SolidWorks Electrical cabinet layouts: items are sorted by block name (rail, duct, plate) since the database has no item kind; a footprint without a box size takes its catalog part's width and height, else 18 x 90 mm; upright rails are left out (WiringCraft rails run left to right) | `crates/wc-import/src/swe/cabinet.rs` | #59 (sizes from the DWG blocks) |
| Binary DXF | `crates/wc-import/src/dxf.rs` | Not planned: the error tells the user to save as ASCII DXF, which every CAD program can do. |
| DWG | `docs/manufacturer-data.md` | Not planned for now: closed format, and makers offer DXF next to it. SolidWorks Electrical DWGs are #59. |
| STEP (3D) | `docs/manufacturer-data.md` | Later: 3D belongs to the one-way export to SolidCraft (phase 5). |

## Editing and app

| Gap | Todo |
|---|---|
| A page's own title-block fields (revision, date, checked by) only come from an import; the app, CLI and assistant cannot edit them | #169 |
| Wire route styles | #52 (pull request #87) |
| Delete in the wire right-click menu | #88 |
| Cover page and common page template editing | #53 |
| Row letters and grid coordinates | #54 (pull request #90) |
| Assistant runs slow tools on the window thread | #67 |

## Shared sessions

The core (#79), the network transport (#74), discovery (#75) and the app's Share session and
Join session (#76) are in. In a session the app sends every change to the open page as one page
edit and project-level changes (devices, comments, panels, pages, project properties) as project
edits; Undo is off. Tools that change pages nobody else has open (wire numbering, a tag renamed
on every page) send those pages together as one change (#77); wire numbering leaves pages other
people hold as they are and says which. Still open: undo per change (#78). #43 tracks it.

## Planning and integrations

| Gap | Todo |
|---|---|
| Full EPLAN / SolidWorks Electrical feature list with triage | #55 ([feature-matrix.md](feature-matrix.md)) |
| WAGO marking and configurator workflow | #57 |
