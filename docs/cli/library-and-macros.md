# CLI: library, macros, template, logo and translations

A short reference for `wiringcraft lib`, `macro`, `template`, `logo` and `translations`, with
examples. Every command takes `-C <dir>` (or `--project <dir>`) for the project folder; the
default is the current folder. `wiringcraft <command> --help` gives the full option list.

The examples start from a new project with the repository's `library/` folder as its source.
Run them from the folder that holds your `wiringcraft` checkout, so that `demo` is made beside it
and `../wiringcraft/library` points at the library from inside `demo`. From anywhere else, give
`lib add` the path to the checkout's `library/` folder instead:

```sh
wiringcraft new demo
cd demo
wiringcraft lib add std folder ../wiringcraft/library
wiringcraft page add power --number 2 --title Power
wiringcraft place power wc.breaker.1p --at 50,50 --tag -F1
wiringcraft device -F1 --part example.ex-mcb-1p-c10
```

## How the library reaches a project

A library source is a folder with `symbols/<id>.json` and `parts/<id>.json` (and optionally
`macros/` and `rules.toml`). The sources are listed as `[[library]]` tables in `project.toml`.
When two sources hold the same id, the one listed first wins.

A project does not draw from the sources directly. It works from its own copies:

- `vendor/symbols/<id>.json` and `vendor/parts/<id>.json` are copies of every library item the
  project uses.
- `library.lock` pins each of them: the source it came from and a hash of the copied version.

```toml
[symbols."wc.breaker.1p"]
source = "std"
hash = "sha256:acb0399b93028272"

[parts."example.ex-mcb-1p-c10"]
source = "std"
hash = "sha256:ffc973488624446c"
```

`place` and `device --part` copy and pin an item the first time it is used. After that, a change
in the library does not reach the project until you take it with `lib update`. So a project
opens and prints the same on any computer, also without the library or a network. Commit
`library.lock` and `vendor/` with the rest of the project.

## lib

| Command | What it does |
|---|---|
| `lib add <name> <kind> <location>` | Add a source. `kind` is `folder` or `git`. A source is added after the existing ones, so it loses to them. `--reference <ref>` picks a git branch, tag or commit (default: the default branch). |
| `lib sync` | Clone or refresh the git sources. Folder sources are listed as they are. |
| `lib status` | Compare every used or pinned item with the sources. |
| `lib update [ids...]` | Take the library's newer version of the changed items, or only of the ids given. |
| `lib lock` | Pin used items that are not pinned yet, and write a pinned item's vendored copy again when it is missing or unreadable. |
| `lib search <query>` | Find symbols and parts by id, name, category, letter code or description. |
| `lib propose <source> <id>` | Copy the project's version of an item to `<source>/proposed/` for review. `--part` when the id is a part. |

There is no command to remove or reorder sources: edit the `[[library]]` tables in
`project.toml`. A relative folder location is read from the project folder.

### Sources

```sh
wiringcraft lib add shared git https://example.com/our-library.git --reference v2
wiringcraft lib sync
```

```text
std: folder /home/me/wiringcraft/library
shared: cloned https://example.com/our-library.git
```

Git sources are cloned into a cache folder, `<cache>/git/<name>`, where `<cache>` is
`$WIRINGCRAFT_CACHE`, else `$XDG_CACHE_HOME/wiringcraft`, else `~/.cache/wiringcraft`. Run
`lib sync` once after adding a git source: until then its items are not found. `lib sync` exits
with an error when any source could not be synced.

```sh
wiringcraft lib search "button no"
```

```text
symbol  wc.button.no                     std          Push button, make
part    example.ex-pb-grn-no             std          Example EX-PB-GRN-NO Push button green, 1 NO (example data)
```

An item matches when it holds every word of the query, in any order and any case. The query is
one argument, so quote it when it has more than one word.

### Status and update

```sh
wiringcraft lib status
```

```text
symbol wc.breaker.1p: CHANGED in library; used by 2
    name: "Circuit breaker, 1-pole" -> "Circuit breaker, 1-pole (new)"
part example.ex-mcb-1p-c10: up to date; used by -F1
```

Each line is the item, its state and the page numbers (symbols) or device tags (parts) that use
it. The
states are:

- `up to date`: the library holds the pinned version.
- `CHANGED in library`: the library holds another version. The lines under it list the
  differences.
- `not pinned`: used but not in `library.lock` (for example after a hand edit). `lib lock` pins it.
- `gone from library (using project copy)`: no source holds it any more. The vendored copy keeps
  working.
- `library file declares another id (not taken)`: the library file holds a different id. Fix
  the library file.

`lib status` only reports; it always exits without an error.

```sh
wiringcraft lib update wc.breaker.1p
```

```text
updated 1 item(s)
```

Without ids, every changed item is updated. An id the project neither uses nor pins is an
error, and nothing is updated.

### Lock

```sh
rm vendor/parts/example.ex-mcb-1p-c10.json
wiringcraft lib lock
```

```text
restored the vendored copy of part example.ex-mcb-1p-c10
```

`lib lock` never takes a newer version: a missing copy is written again only at the pinned
version, and a library holding another version is left to `lib update`. It prints nothing when
it only pins new items. An item it cannot pin or restore is printed as a warning, for example
`warning: part example.ex-mcb-1p-c16 is in no library source`.

### Propose

```sh
wiringcraft lib propose std wc.breaker.1p
wiringcraft lib propose std example.ex-mcb-1p-c10 --part
```

```text
proposed /home/me/wiringcraft/library/proposed/symbols/wc.breaker.1p.json
proposed /home/me/wiringcraft/library/proposed/parts/example.ex-mcb-1p-c10.json
```

The real item in the source is not changed; a librarian (or a pull request) moves it in.

## macro

A macro is a saved group of symbols, wires and drawings, one JSON file in the `macros/` folder of
the project or of a library source. Macros are made in the app; the CLI lists, shows, edits and
places them. Texts in a macro can hold placeholders, `{{name}}`, filled from a named value set
(one per variant) when the macro is placed.

| Command | What it does |
|---|---|
| `macro list` | The macros, project first, with their value sets. |
| `macro show <name>` | Description, placeholders, value sets, the placeholders its texts use, and a problem `edit` would refuse to save. |
| `macro edit <name> [options]` | Change a project macro's description, placeholders and value sets. A library's macros are not edited here. |
| `macro place <page> <name> --at x,y` | Place it with its top-left corner at `--at` (mm). `--set <set>` picks the value set (needed when it has any); `--value name=value` (repeatable) overrides one value. The symbols it uses and the parts its devices and accessories name are copied into the project and pinned, as `device --part` does (a part no library has stays a plain name). When that would bring back a symbol the project already places but has no readable copy of, and a strip with jumpers or a rail order would be numbered differently with it, nothing is placed: restore the file first. Prints the new symbol ids. |

`macro edit` options, all repeatable except `--description`:

- `--description <text>`
- `--placeholder name[:default[:description]]` declares a placeholder or changes one.
- `--rename old=new` renames a placeholder everywhere in the macro.
- `--remove <name>` removes a placeholder; its texts must no longer use it.
- `--set set:name=value` sets a value, adding the value set when it is new.
- `--remove-set <set>`

The macro is written only when the result checks out (for example, every placeholder its texts
use is declared).

The example below needs a project macro named `feeder`. `macro edit` does not create one (it
stops with `no macro "feeder"`), so first make it in the app: select the breaker on page `power`,
use "Save as macro" and name it `feeder`. The commands then change that macro's
description, placeholders and value sets.

```sh
wiringcraft macro edit feeder --description "Breaker feeder" \
  --placeholder "circuit:L1:Circuit name" --placeholder breaker \
  --set c10:breaker=example.ex-mcb-1p-c10 --set c16:breaker=example.ex-mcb-1p-c16
wiringcraft macro list
```

```text
project	feeder	Feeder  [c10, c16]
```

```sh
wiringcraft macro place power feeder --at 100,50 --set c10 --value circuit=Pump
```

```text
s2
```

The symbols get the next free tags (here `-F2`), and the devices get the macro's data with the
placeholders filled (`Feeder Pump`). Symbols the project does not have yet are copied and pinned,
and so are the parts the macro's devices and accessories name. A part no library source has stays
a plain name on the device; `check` reports it.

## template

The page template: the number of reference columns across the frame, and the boxes of the title
block. It is stored as `[template]` in `project.toml`; a project without one uses the built-in
layout.

```sh
wiringcraft template show        # print the template as TOML
wiringcraft template columns 12  # 1 to 40 reference columns
wiringcraft template reset       # remove [template], back to the built-in layout
```

`template columns` writes the whole template (columns and boxes) into `project.toml`. The boxes
have no command of their own: edit the `[[template.box]]` tables there, starting from
`template show`. `columns` and `reset` print nothing on success.

## logo

The project logo, shown on the cover page and in every title block.

```sh
wiringcraft logo set ~/Pictures/company.png
wiringcraft logo show
```

```text
pictures/company.png
```

`logo set` takes a PNG or JPEG file and copies it into the project's `pictures/` folder (a
picture already in that folder is used where it is). The logo's place on the cover page is kept.
`logo remove` takes the logo off the cover page and the title blocks; the file stays in
`pictures/`. `logo show` prints `no logo` when there is none.

## translations

The project's translation dictionary, `translations.toml` in the project folder. The texts it
covers are page titles, device and location descriptions and free texts on pages. Tags, wire
numbers, part numbers and title-block fields are not translated. `--lang` is needed by every
form of the command.

```sh
wiringcraft translations --lang nb
```

```text
Cover	page 1 title
Feeder Pump	-F2 description
Power	page 2 title
error: 3 text(s) have no translation into nb
```

Without a subcommand it lists the texts with no translation into the language, with where each
is used, and exits with an error when any is missing, so it can guard a release or a CI job.
When none is missing it prints `every text has a translation into nb`.

```sh
wiringcraft translations --lang nb set "Power" "Kraft"
wiringcraft translations --lang nb remove "Power"
```

The text is given as drawn, in the source language. `set` refuses an empty translation, and
`remove` is an error when the text has no translation into that language. To print in a
language, pass `--lang` to `pdf` (or `drawing`), for example `--lang nb`, or `--lang nb,en` for
each text with the second language after it.
