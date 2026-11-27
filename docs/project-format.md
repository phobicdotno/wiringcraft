# Project folder format

A WiringCraft project is a plain folder of text files: TOML for settings and JSON for drawings.
It is made to live in git. `wiringcraft new` creates the folder and makes it a git repository
(`--no-git` leaves that out).

## Layout

```text
my-project/
  project.toml                      name, title block, library sources and project settings
  devices.json                      device tag -> part, description and properties
  library.lock                      exact version of every library item in use
  pages/<id>.page.json              one file per page
  panels/<id>.panel.json            one file per cabinet panel layout
  comments/<page id>.comments.json  review comments on one page
  vendor/symbols/<id>.json          copies of the library symbols in use
  vendor/parts/<id>.json            copies of the library parts in use
  macros/<id>.json                  the project's own macros
  translations.toml                 translation dictionary
  pictures/                         the logo and other pictures
  .gitattributes, .gitignore        written by `wiringcraft new` unless --no-git
```

Only `project.toml` is needed. A missing `devices.json` or `library.lock` reads as empty, and a
missing folder as one with nothing in it. A new project made without `--template` has `project.toml`, `devices.json`,
`library.lock` and the cover page `pages/p0001.page.json`, plus the two git files when
`wiringcraft new` sets up git (not with `--no-git`; a later plain `git init` does not add them). The other files
and folders appear when something is first put in them. A project made with `--template` instead
starts with a copy of everything in the base project except its `.git` and `comments/` folders:
its pages rather than `p0001`, and its own `.gitignore` and `.gitattributes` if it has them, even
with `--no-git`. `wiringcraft new` then adds either git file only where the copy has none.

## project.toml

The project's name, format version, title block and library sources, followed by one TOML
table array for each kind of project setting. A table array that would be empty is left out.

```toml
name = "demo"
format_version = 1

[title_block]

[[library]]
name = "std"
kind = "folder"
location = "/home/user/wiringcraft/library"
```

| Key or table | What it holds | Command |
|---|---|---|
| `name` | the project's name | `new --name` |
| `format_version` | the file format's version, now 1 | |
| `[title_block]` | fields printed on every page: `customer`, `drawn_by`, `revision`, ... (a page's own fields, set with `page fields`, win on that page) | |
| `[[library]]` | library sources, earlier ones win: `name`, `kind` (`folder` or `git`), `location`, and for git an optional `reference` | `lib add` |
| `[[location]]` | cabinets, rooms and panels (the `+` part of device tags): `path`, `description` | `location` |
| `[[jumper]]` | jumpers between terminals of one strip: `strip`, `terminals`, optional `part` | `jumper` |
| `[[strip]]` | terminal order and spare terminals of strips arranged in the terminal strip editor | `strip` |
| `[logo]` | the logo's `file` (a path inside the project) and its place on the cover page | `logo` |
| `[template]` | the frame's columns and the title block's layout, only once it differs from the built-in one | `template` |
| `[[revision]]` | issued revisions, oldest first: `letter`, `date`, `text`, `author` | `revision` |
| `[[report]]` | report layouts the user designed, one per report kind | `report-layout` |
| `[[rule]]` | the project's own check rules | `rules set` |
| `rules_off` | library rules this project turns off, as `source:number` or `source:name` | `rules off` |
| `[[rack]]` | PLC racks and the cards in their slots (`[[rack.card]]`) | `rack` |
| `[[network]]` | fieldbus networks: their devices, addresses and cables ([docs](networks.md)) | |

Examples from a scratch project:

```toml
[[revision]]
letter = "A"
date = "2026-11-27"
text = "First issue"
author = "doc"

[[report]]
kind = "parts"
title = "Parts list"

[[report.column]]
from = "Qty"
heading = "Qty"

[[rule]]
kind = "cable"
devices = "-M*"

[[rack]]
name = "-A1"
description = "Main PLC"

[[rack.card]]
slot = 1
type = "DI 16x24 V DC"
channels = 16
start = "%IX0.0"
```

A key a `[[rule]]` table does not know (a misspelt `severty`) is kept and written back, and
`check` reports it.

A project whose `format_version` is newer than this WiringCraft reads is refused, with a message
that names the file and asks to update WiringCraft.

## devices.json

One entry per device, keyed by device tag. Two spellings of one tag (`-K1+X1` and `+X1-K1`) are
one device, and a file with both is refused.

```json
{
  "-K1": {
    "description": "Pump contactor",
    "part": "example.ex-ct-9-24dc"
  },
  "-S1": {}
}
```

## library.lock and vendor/

Every library symbol and part the project uses is copied into `vendor/symbols/` or
`vendor/parts/`, and pinned in `library.lock` with the source it came from and a hash of its
content. The project is drawn from these copies, so it looks the same on every machine whatever
the library sources hold now. `lib status` compares the pins with the libraries and `lib update`
takes newer versions.

```toml
[symbols."wc.coil"]
source = "std"
hash = "sha256:bbb7d6d703cde76e"

[parts."example.ex-ct-9-24dc"]
source = "std"
hash = "sha256:c349eeb58508cd21"
```

A black box drawn with `box` is a symbol of the project itself, saved as
`vendor/symbols/box.<page>.<id>.json`. A save removes such a file when no page places it and the
lock does not pin it. Other vendored files are never removed by a save.

## pages/

One file per page, named after the page id: `pages/p0002.page.json`. The file holds the page's
number, title, sheet size, the symbols placed on it and its wires. A file whose `id` differs from
its file name is refused. Deleting a page removes its file.

```json
{
  "id": "p0002",
  "number": "2",
  "size": "A3",
  "symbols": [
    {
      "at": { "x": 100.0, "y": 100.0 },
      "id": "s1",
      "symbol": "wc.coil",
      "tag": "-K1"
    }
  ],
  "title": "Main power",
  "wires": [
    {
      "from": { "item": "s2", "pin": "14" },
      "id": "w1",
      "number": "101",
      "to": { "item": "s1", "pin": "A1" }
    }
  ]
}
```

(Shortened here; the real file puts each key on its own line.)

Page and panel ids become file names, so they are 1 to 128 of `A-Z a-z 0-9 . _ -`, do not
start with `.`, and do not start with a name Windows keeps for a device (`con`, `nul`, `aux`,
`com1`, ...). Two ids that differ only in case are refused, since on macOS and Windows they
would be one file.

## panels/

One file per cabinet panel layout, `panels/<id>.panel.json`: the plate's size and its rails,
ducts and devices. As with pages, the `id` inside must match the file name.

## comments/

Review comments, one file per page: `comments/<page id>.comments.json`. Keeping them per page
lets people review different pages and merge in git without conflicts. The file name says which
page a comment is on. Comments on a deleted page go with it.

```json
[
  {
    "at": { "x": 100.0, "y": 100.0 },
    "author": "doc",
    "id": "c1",
    "page": "p0002",
    "text": "Check coil voltage",
    "time": 1795761766
  }
]
```

## macros/

The project's own macros, one JSON file each: `macros/<id>.json`. A macro is a saved group of
symbols, wires and drawings, with optional placeholders and value sets. A library source can
have a `macros/` folder of the same kind. `macro list` lists both, project first.

## translations.toml

The translation dictionary. `source` is the language the project is drawn in (left out means
`en`), `languages` lists languages added before any text has a translation in them, and each
`[[text]]` table holds one text in each language.

```toml
[[text]]
en = "Main power"
nb = "Hovedstrom"
```

## pictures/

`logo set` copies the logo picture (PNG or JPEG) here, and `[logo]` in `project.toml` names it,
such as `pictures/logo.png`. A different picture of the same name gets a new name
(`logo-2.png`), so the old one stays for undo.

## Files that are not part of the project

- `.wiringcraft.lock`: locked while a `wiringcraft` command works on the project, so commands
  take turns. The lock is best effort: when it cannot be opened or taken, the command prints a
  warning and runs without it, so do not run another command on the project until it is done.
  The new project's `.gitignore` lists it, and in a repository whose `.gitignore` does not, the
  next command puts it in git's local exclude list (a warning says when that fails). A project
  made with `--no-git` has neither, so after a plain `git init`, `git add .` stages the lock file
  unless a command has run in between; add `/.wiringcraft.lock` to `.gitignore` to be sure.
- `translations.toml.lock.tmp`: locked while `translations.toml` is read and changed.
- `*.tmp`: staging files of a write in progress.
- `export/`: listed in `.gitignore` as generated output.
- `.box-edit.journal`: kept while `box-edit` writes. One left behind means a run stopped half
  way, and the next `wiringcraft` command in the project puts the files back from it.

The `.gitignore` of a new project:

```text
# WiringCraft temporary and generated files
*.tmp
/export/
/.wiringcraft.lock
```

## Git-friendly writing

Every project file is written so that a diff shows real changes only:

- **Fixed form.** JSON keys come out sorted, with two-space indentation and a trailing newline.
  Maps in TOML (title block fields, lock entries, translations) are sorted by key. Pages,
  panels and comments are kept in a fixed order.
- **Only when changed.** A file that already holds the new content is not written, so its
  modification time stays and git sees nothing.
- **Atomic.** Each project file is written to `<name>.tmp`, synced to disk, then renamed over
  the old file. A crash leaves the old file or the new one, never half of one. On Unix the folder
  is also synced after the rename, so the rename survives a power loss; a folder that cannot be
  synced (the file system does not support it, or the folder cannot be read) is accepted without
  it. On other systems the folder is not synced.
  Pictures in `pictures/` are the exception: `logo set` writes a new picture straight to its own
  new name, so a crash can leave a partial picture there (the old one is never touched).
- **LF line endings.** The new project's `.gitattributes` makes the text files LF everywhere, so
  Windows and Linux produce identical files:

```text
*.json text eol=lf
*.toml text eol=lf
library.lock text eol=lf
*.pdf binary
```
