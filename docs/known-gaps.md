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
| QElectroTech `.elmt` elements are not read | `docs/manufacturer-data.md` | #91 |
| EDZ exchange files (EPLAN macros and parts) are not read | `docs/manufacturer-data.md` | #93 (parts first; macros follow) |
| DXF: ELLIPSE, SPLINE and HATCH are skipped | `crates/wc-import/src/dxf.rs`, `docs/manufacturer-data.md` | #95 |
| KiCad: only the first unit of a multi-unit symbol; `.kicad_sym` inside a zip is listed, not imported | `crates/wc-import/src/kicad.rs`, `crates/wc-import/src/archive.rs` | #96 |
| SolidWorks Electrical: symbol and page graphics from DWG | `crates/wc-import/src/swe.rs` | #59 |
| SolidWorks Electrical: cabinet layouts, cover page, library parts, own symbols | `crates/wc-import/src/swe.rs` | #60, #61, #62, #71 |
| SolidWorks Electrical: terminal jumpers made in the terminal strip editor are counted in the import report, not imported (WiringCraft has no jumpers yet) | `crates/wc-import/src/swe.rs` | #108 |
| SolidWorks Electrical: tag paths stored with spaces instead of `=`/`+`/`-` lose their function and location | `crates/wc-import/src/swe.rs` | #114 |
| SolidWorks Electrical: a component keeps only its first part ("extra part(s) left out") | `crates/wc-import/src/swe.rs` | #106 (several parts per device) |
| PDF trace: pictures compressed other than Flate or DCT, or not 1-bit gray, 8-bit gray or 8-bit RGB, are refused ("not supported yet") | `crates/wc-import/src/pdf.rs` | To file next (todo limit per run reached) |
| Binary DXF | `crates/wc-import/src/dxf.rs` | Not planned: the error tells the user to save as ASCII DXF, which every CAD program can do. |
| DWG | `docs/manufacturer-data.md` | Not planned for now: closed format, and makers offer DXF next to it. SolidWorks Electrical DWGs are #59. |
| STEP (3D) | `docs/manufacturer-data.md` | Later: 3D belongs to the one-way export to SolidCraft (phase 5). |

## Editing and app

| Gap | Todo |
|---|---|
| Wire route styles | #52 (pull request #87) |
| Delete in the wire right-click menu | #88 |
| Cover page and common page template editing | #53 |
| Row letters and grid coordinates | #54 (pull request #90) |
| Show and edit locations | #94 |
| Assistant runs slow tools on the window thread | #67 |

## Shared sessions

`README.md` says of `wc-share`: "app and network to follow". The core is merged (#79); the rest is
#74 (network transport), #75 (discovery), #76 (app UI), #77 (project-level edits from page
tools) and #78 (per-change undo). #43 tracks them.

## Planning and integrations

| Gap | Todo |
|---|---|
| Full EPLAN / SolidWorks Electrical feature list with triage | #55 ([feature-matrix.md](feature-matrix.md)) |
| WAGO marking and configurator workflow | #57 |
