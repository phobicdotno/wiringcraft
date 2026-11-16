# WiringCraft

An open, original electrical schematic and wiring design tool written in Rust.

WiringCraft covers the familiar workflow of electrical CAD (schematic pages, symbols, device tags,
wires and potentials, terminal strips, cables and reports) and rebuilds the plumbing underneath it:

- **Projects are plain text.** One file per page, sorted keys, written atomically and only when
  changed. Diff, merge and review them in git like code.
- **The library is versioned.** Every symbol and part has a content hash; a project pins the exact
  versions it uses in `library.lock` and keeps a copy in `vendor/`, so it opens offline and never
  changes underneath you. Updates are shown as a before/after list you accept item by item.
- **Everything is scriptable** through the `wiringcraft` command line.

## Status

Phase 1 (foundation) is in progress: project format, library and sync, imports, PDF export.
See the feature list for the full plan.

## Layout

| Crate | Role |
|---|---|
| `wc-core` | data model, device tags (IEC 81346 style), project folder format, connectivity |
| `wc-library` | library sources, lockfile, vendoring, update status |
| `wc-import` | DXF, SVG and CSV import |
| `wc-pdf` | vector PDF export |
| `wc-cli` | the `wiringcraft` command |

## Build

```sh
cargo test --workspace
cargo run -p wc-cli -- --help
```

## Rules

- Original work only: no code, symbols, icons, templates or branding copied from other products.
- No panics on user input: no `unwrap`/`expect`/`panic!` outside tests (enforced by clippy lints).
