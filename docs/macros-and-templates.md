# Macros and templates

WiringCraft has three ways to reuse work: macros (a group of symbols and wires you insert again
and again), base projects (a whole project you start new projects from) and the page template
(the frame and title block every page is drawn on, with the project logo). This page explains
how each one works. The commands are listed in
[docs/cli/library-and-macros.md](cli/library-and-macros.md); `wiringcraft <command> --help`
gives the same.

## Macros

A macro is a saved group of symbols, wires and free drawings (lines, rectangles, texts), with
the device data of its symbols (description, part, accessories, properties). Each macro is one
JSON file in a `macros` folder:

- `<project>/macros/<id>.json` for the project's own macros.
- `<source>/macros/<id>.json` in a library source, for macros shared by every project that uses
  the source.

The file id comes from the macro's name: lower case letters, digits and dashes, so
"Motor starter, DOL 3~" is saved as `motor-starter-dol-3.json`. Lists show the project's macros
first, then each library source's. When a command names a macro, a file id match wins over a
name match, and the project's macro wins over a library macro with the same id.

### Making a macro

In the app, select the symbols, wires and drawings, then choose Insert > Save as macro and give
it a name. It is saved in the project's `macros` folder. Wire numbers are left out, because a
number belongs to one drawing, not to the reusable group. Library macros are made the same way
in a project and then copied into the library source's `macros` folder.

The command line cannot save a new macro; it lists, shows, places and edits them.

### What placing a macro changes

Placing a macro works like a paste:

- Every symbol gets a fresh id on the page.
- Every device tag is replaced by the next free tag with the same function, location and letter
  code. A macro drawn with `-K1` is placed as `-K2` when `-K1` is taken, then `-K3`, and so on.
- The placed devices get the macro's device data (description, part, properties), not the data
  of a device in the project that happens to have the old tag.
- Wires follow their symbols, keep their cross-section, colour and type, and have no number.
  Number them afterwards with `number-wires` or by hand.
- Drawings move along with the group.

The macro's anchor (the top-left corner of the group when it was saved) lands where you place
it. In the app the group is inserted where it was drawn and left selected, so you drag it into
place. `macro place --at x,y` puts the anchor at that point instead.

When no free tag is left (all of 1 to 9999 are taken), the command line places nothing and says
which tag ran out; the app inserts those symbols without a tag.

Symbols the macro uses are taken from the library and pinned in `library.lock`, as `place` does.
A symbol already pinned keeps its pinned version.

### Placeholders and value sets

A macro can leave values open as placeholders, written `{{name}}`. They can be used in:

- the devices' descriptions, parts, accessories and properties,
- the wires' cross-section, colour and type,
- the macro's drawn texts.

Device tags cannot hold placeholders; tags are renumbered as described above.

Each placeholder is declared with a name, an optional default and an optional description. A
value set is a named variant that gives values to some or all placeholders, so one macro covers
several sizes (a motor starter for 0.75, 1.5 and 3 kW). When the macro is placed:

- A macro with value sets needs one chosen (`--set` on the command line, Insert… and the
  variant in the app). A macro without value sets uses the placeholders' defaults.
- A placeholder the chosen set does not give takes its default.
- On the command line, `--value name=value` sets one placeholder over the value set.
- Every `{{name}}` is then replaced. A placeholder left without a value is an error and nothing
  is placed.
- A wire value or a part that comes out empty is dropped, so a variant can leave out a part or
  an accessory.

Placeholder and value set names are matched without regard to case. Before a macro is saved or
placed it is checked: names must be unique and not empty, value sets may only fill declared
placeholders, and every placeholder used in the texts must be declared.

Placeholders and value sets of a project macro are edited in the app (Insert > Macro, then
Placeholders…) or with `macro edit`. Renaming a placeholder also renames its uses in the
macro's texts. Library macros are edited in the library, not from a project.

An example, with a project macro `relay` whose coil description is `Relay {{coil}}` and whose
wire cross-section is `{{section}}`:

```
$ wiringcraft macro list
project	relay	Relay with feed  [fine, heavy]
$ wiringcraft macro place p2 relay --at 100,50
error: macro Relay with feed: choose a value set: fine, heavy
$ wiringcraft macro place p2 relay --at 100,50 --set heavy
s2 w2
$ wiringcraft device -C . -K2
part         -
description  Relay 230 V AC
```

The macro was drawn with `-K1`; it was placed as `-K2` because the page already had a `-K1`.

### Pages from a table

A table can drive a macro: `wiringcraft macro generate <macro> --table motors.csv` places one
copy per row, each row's cells filling the placeholders (a column `set` picks the value set), on
new pages. See [Pages from a table](cli/library-and-macros.md#pages-from-a-table).

### EPLAN macros

The macros in an EPLAN parts archive, read by `import parts-edz`, become library symbols
(`eplan.<maker>.<macro name>`), not WiringCraft macros. See
[docs/eplan-import.md](eplan-import.md).

## Base projects

A base project (a project template) is an ordinary project folder that new projects start
from, usually a company's standard: its settings, title-block fields, page template, cover page,
standard pages, library sources, logo and macros.

```
wiringcraft new /work/pump-station --name "Pump station" --template /work/base-project
```

`--template` takes a project folder or a git repository URL. Everything in the base project is
copied except its review comments, its git history and symbolic links (a linked file or
folder, such as a `macros` folder that is a link, is left out). Then:

- `{name}` and `{date}` in the title-block fields and page titles become the new project's name
  and today's date. A base page titled `{name} control` becomes `Pump station control`.
- The project's `date` field is set to today, and the pages' own dates are dropped.
- Library sources keep pointing at the same libraries. A folder source inside the base project is
  copied along; a relative folder outside it (such as `../library`) is written as the absolute
  folder it named. That works only for a base project in a folder: a base project fetched from
  a git repository cannot have a relative folder source that leads outside the repository
  (such as `../library`), and `new` stops with an error. Absolute folders and git sources are
  fine there. Start from a local copy of it, or point those sources at absolute folders or git.

## Page template and title block

The page template is the frame and title block that every page of a project is drawn on, in the
PDF, the drawing exports and the app. It sets:

- the number of reference columns across the frame (1 to 40), which cross-references and wire
  numbering by column also use,
- the height of the title-block band (10 to 60 mm, 24 mm by default),
- the title block's boxes: each has a caption, edges measured in millimetres from the left or
  right side of the frame (so boxes at the right stay at the right on every sheet size), and the
  values written in it.

A box text shows one or more values, joined with ` · `: the project's title-block fields
(`customer`, `project_number`, `plant`, `location`, `drawn_by`, `checked_by`, `revision`,
`date`, `description` or any other key the project has) or the special values `project.name`,
`page.title`, `page.number` and `page.sheet` (`3 / 12`). A page can have its own values for the
fields (`page fields`, such as revision or date), which win over the project's on that page.

The template is stored under `[template]` in `project.toml`. A project without it draws the
built-in layout, so the section is only written once you change the layout. Edit it in the app
with Page template, or see it with `wiringcraft template show`. To share a title block between
projects, keep it in a base project.

### Logo

A project has at most one logo, a PNG or JPEG file. `wiringcraft logo set <file>` (or the
Cover page properties in the app) copies it into the project's `pictures` folder. The logo is
drawn in the title-block box marked as the logo box on every page (the built-in layout has one at
the right), and larger on the cover page, where you put it. Removing the logo takes it off the
pages; its file stays in `pictures`.
