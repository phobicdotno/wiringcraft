# Library source format

A library source is a folder of symbols and parts that projects take their copies from. The
repository's own `library/` folder is one. This page describes what such a folder holds and how
WiringCraft reads it. For the commands that add sources, search them and take updates, see
[docs/cli/library-and-macros.md](cli/library-and-macros.md).

## Layout

```text
<source>/
  symbols/<id>.json     one symbol per file
  parts/<id>.json       one part per file
  macros/<id>.json      optional: macros, offered by `macro list` and `macro place`
  rules.toml            optional: house check rules for every project using the source
  proposed/             written by `lib propose`: items offered back for review
```

The repository's `library/` holds `symbols/` (75 starter symbols) and `parts/` (17 example
parts). It has no `macros/` folder or `rules.toml`.

A file's name without `.json` is the item's id, and the id inside the file must be the same.
An id is 1 to 128 characters of `A-Z a-z 0-9 . _ -`, does not start with `.`, and does not
start with a name Windows keeps for a device (`CON`, `NUL`, `COM1` and so on). Only `.json`
files in `symbols/` and `parts/` are listed. A library file that declares another id than
its file name is reported by `lib status` and is not taken into a project.

A source is listed in a project's `project.toml` as a `[[library]]` table:

```toml
[[library]]
name = "std"
kind = "folder"
location = "/path/to/library"
```

`kind` is `folder` (a local or network folder; a relative `location` is relative to the
project folder) or `git` (a repository URL, with an optional `reference` branch, tag or commit).
A git source is cloned with depth 1 into the cache folder `git/<name>`, where the cache folder is
`$WIRINGCRAFT_CACHE`, else `$XDG_CACHE_HOME/wiringcraft`, else `~/.cache/wiringcraft`. Sources
listed first win when two of them hold the same id.

All files are JSON (UTF-8). WiringCraft writes them pretty-printed with sorted keys and a final
newline. Lengths are in millimetres and the y axis points down.

## Symbol files

A symbol is the drawing of one device function with named pins. It is read into
`wc_core::Symbol` (`crates/wc-core/src/symbol.rs`).

| Field | Required | Meaning |
|---|---|---|
| `id` | yes | Stable id, such as `wc.contact.no`. Never reuse an id for a different meaning. |
| `name` | yes | Name shown in lists and search. |
| `category` | no | Free group name, such as `contacts` or `protection`. |
| `letter_code` | no | Letter suggested for new devices, such as `K` for relays. |
| `role` | no | `main`, `auxiliary`, `standalone` (default), `terminal` or `interruption`. |
| `graphics` | yes | The drawing: a list of shapes (below). |
| `pins` | yes | The connection points: `name`, `at` and `dir`. |
| `tag_at` | no | Where the device tag is printed, relative to the origin. Default `0,0`. |
| `description` | no | Longer text, also searched. |
| `source` | no | Set by importers: the item's identity in the library it was imported from. |

The role links symbols with the same device tag: a `main` symbol (a coil) drives the
`auxiliary` symbols (its contacts) and gets their cross-references; a `terminal` is a terminal of
a strip; `interruption` points with the same tag continue one wire, usually on another page.

A pin has a `name` (the designation printed next to it, such as `13`, `A1` or `L1`), a point
`at` relative to the symbol's origin, and `dir`, the way a wire leaves it: `up`, `down`
(default), `left` or `right`. Pin names are what wires attach to (`s1:13`). The starter
symbols draw their pin labels as `text` shapes in `graphics`.

Each shape in `graphics` has a `kind`:

| `kind` | Fields |
|---|---|
| `line` | `a`, `b` |
| `polyline` | `points`; optional `closed` and `filled` (filled with the line colour, and closed) |
| `rect` | `min`, `max` |
| `circle` | `center`, `radius`; optional `filled` |
| `arc` | `center`, `radius`, `start`, `end` in degrees, clockwise as seen on paper |
| `text` | `at`, `text`; optional `height` (default 2.5), `align` (`left` default, `center`, `right`), `angle` (degrees, counter-clockwise on paper) |
| `dimension` | `a`, `b`; optional `offset` and `height` |

Points are `{"x": ..., "y": ...}`. The starter symbols are drawn on a 5 mm grid with the origin
at the top pin. Here is `library/symbols/wc.contact.no.json`, with two of its five shapes left
out:

```json
{
  "category": "contacts",
  "description": "Auxiliary make contact, driven by the coil with the same tag.",
  "graphics": [
    {
      "a": { "x": 0.0, "y": 10.0 },
      "b": { "x": -3.5, "y": 4.2 },
      "kind": "line"
    },
    {
      "at": { "x": 1.2, "y": 2.5 },
      "height": 2.0,
      "kind": "text",
      "text": "13"
    }
  ],
  "id": "wc.contact.no",
  "letter_code": "K",
  "name": "Normally open contact",
  "pins": [
    { "at": { "x": 0.0, "y": 0.0 }, "dir": "up", "name": "13" },
    { "at": { "x": 0.0, "y": 15.0 }, "dir": "down", "name": "14" }
  ],
  "role": "auxiliary",
  "tag_at": { "x": -6.0, "y": 8.0 }
}
```

### Starter symbols and their generator

Every file in `library/symbols/` is written by `tools/gen_starter_symbols.py`: one `sym(...)`
call per symbol, with helpers for the shapes and the usual pin labels. Run it from the
repository root:

```sh
python3 tools/gen_starter_symbols.py
```

CI deletes `library/symbols/*.json`, runs the script and fails when the result differs from
what is committed. So a starter symbol added or changed by hand must be added to the script too.

## Part files

A part is an orderable article. It is read into `wc_core::Part`
(`crates/wc-core/src/part.rs`). Only `id` is required.

| Field | Meaning |
|---|---|
| `id` | Stable id, usually `<manufacturer>.<order number>` in lower case with other characters as `-` (`Acme Corp`, `AB 12/34` gives `acme-corp.ab-12-34`). |
| `manufacturer` | Maker's name. |
| `order_number` | The maker's order number. |
| `description` | What the part is. |
| `symbol` | Id of the symbol the part is usually drawn with. |
| `properties` | Free text values by name: rated current, coil voltage, sizes and so on. |
| `contacts` | The contacts the part has: `main`, `no`, `nc`, `co` counts. The project check compares a device's drawn contacts with them. |
| `footprint` | The maker's outline drawing for panel layouts: `width`, `height` and `shapes` (the shape kinds above), top-left corner at `0,0`. |
| `model` | The size of the maker's 3D model: `width`, `height`, `depth`. |

`import footprint-dxf` and `import model-step` fill `footprint` and `model`. On a panel layout
a part's width and height come from its `Width` and `Height` properties first, then its
footprint, then its model; its depth from `Depth`, then the model. German and Norwegian names
work too: `Breite` and `Bredde`, `Höhe`, `Hoehe`, `Høyde` and `Hoyde`, `Tiefe` and `Dybde`
(`Width mm`, `Height mm` and `Depth mm` also work). Width and height names are matched without regard to the case
of the letters A to Z only, so write `ö` and `ø` in lower case (`HÖHE` is not found, `HöHE` is).
The leading number of the value is used, so `"45 mm"` and `"45"` are both 45.

`library/parts/example.ex-ct-9-24dc.json`:

```json
{
  "contacts": {
    "main": 3,
    "no": 1
  },
  "description": "Contactor 3-pole 9 A, 1 NO auxiliary, 24 V DC coil (example data)",
  "id": "example.ex-ct-9-24dc",
  "manufacturer": "Example",
  "order_number": "EX-CT-9-24DC",
  "properties": {
    "Coil voltage": "24 V DC",
    "Depth": "80 mm",
    "Height": "85 mm",
    "Rated current": "9 A",
    "Width": "45 mm"
  },
  "symbol": "wc.coil"
}
```

The example parts are made-up data from a maker called `Example`.

## Content hashes and versions

A project never draws from a library source directly. When it first uses a symbol or part, the
item is copied to the project's `vendor/symbols/` or `vendor/parts/` and pinned in
`library.lock` with the source it came from and its content hash (for the parts a placed macro
names, see [Macros](#macros)):

```toml
[symbols."wc.coil"]
source = "std"
hash = "sha256:bbb7d6d703cde76e"

[parts."example.ex-ct-9-24dc"]
source = "std"
hash = "sha256:c349eeb58508cd21"
```

The hash is `sha256:` and the first 16 hex digits of the SHA-256 of the item's canonical JSON
(`crates/wc-core/src/hash.rs`). The item is read into its type first and then written as JSON
with sorted keys and no whitespace. So:

- reformatting a file or reordering its keys does not change the hash;
- a field WiringCraft does not know is dropped when the file is read, so it does not change the
  hash either, and it is not copied into projects;
- any change to a value WiringCraft reads (a pin, a shape, a property) gives a new hash.

There is no version number in the files: the hash is the version. `lib status` compares each
pin with the hash of the file the winning source holds now and reports the item as up to date,
changed (with the differences), not pinned, gone from the library, or as a file that declares
the wrong id. A project keeps its pinned copy until it takes the new version with `lib update`.

## rules.toml

A source can carry house check rules in `rules.toml`. They have the same `[[rule]]` tables as a
project's own rules in `project.toml` and run with `check` for every project that uses the
source, after the built-in checks and before the project's own rules
(`crates/wc-core/src/check_rule.rs`).

```toml
# Every motor has a cable on one of its pins.
[[rule]]
name = "motor-cable"
kind = "cable"
devices = "-M*"

# At most two wires on one terminal pin.
[[rule]]
kind = "max-wires"
max = 2
```

A rule has `kind` (`cable`, `tag` or `max-wires`) and optionally `name`, `devices` (a tag
pattern: `*` any run of characters, `?` one character, `#` a number), `role` (a symbol role),
`pattern` (for `tag`), `max` (for `max-wires`), `severity` (`error`, `warning` default, or
`note`) and `message` (`{tag}` in it is replaced by the device's tag). A rule with an unknown key
or a missing value is reported by the check instead of being skipped.

Library rules are numbered from 1 in file order. A project turns one off by number or by name
in `project.toml`:

```toml
rules_off = ["std:2", "std:motor-cable"]
```

A source whose folder is missing (a git source never synced) or has no `rules.toml` adds no
rules. Rules are read from the source each time and are not pinned in `library.lock`.

## Macros

A macro is a saved group of symbols, wires and drawings, one JSON file per macro in `macros/`
of a project or a library source (`crates/wc-core/src/macros.rs`). The file name is the macro's
id. `macro list` shows the project's macros first, then each source's, and `macro place` finds
a macro by id or by its `name`.

| Field | Meaning |
|---|---|
| `name` | Required. The macro's name. |
| `description` | What it is for. |
| `symbols` | Placed symbols, as on a page: `id`, `symbol`, `at`, optional `rotation`, `mirror`, `tag`. |
| `wires` | Wires between them, as on a page. |
| `graphics` | Free drawing shapes. |
| `devices` | Device data by tag: description, part, properties. |
| `anchor` | The group's top-left corner; placing puts it where you point. |
| `placeholders` | Values left open: `name`, optional `description` and `default`. |
| `value_sets` | Named variants: `name`, optional `description`, and `values` by placeholder name. |

Placeholders are written `{{name}}` in device descriptions, parts, accessories and properties,
in wire cross-sections, colours and types, and in drawn texts. Names are matched trimmed and
without regard to case. A small example:

```json
{
  "name": "Pilot lamp",
  "symbols": [
    { "id": "s1", "symbol": "wc.lamp", "at": { "x": 0.0, "y": 0.0 }, "tag": "-P1" }
  ],
  "devices": {
    "-P1": { "description": "{{colour}} lamp", "part": "example.ex-lamp-24-{{code}}" }
  },
  "placeholders": [
    { "name": "colour", "default": "Green" },
    { "name": "code", "default": "gn" }
  ],
  "value_sets": [
    { "name": "red", "values": { "colour": "Red", "code": "rd" } }
  ]
}
```

Placing a macro gives its symbols fresh ids and the next free tags. Macros themselves have no
hash and are not pinned: they are read from the source when placed. The symbols a placed macro
uses are copied and pinned like any other symbol. The parts named in its `devices` are not
copied or pinned yet (#1418): until `wiringcraft lib lock` is run, `lib status` shows them as not
pinned and `check` warns that they are missing from the project's library copy.
