# Glossary

The words WiringCraft uses, in the meaning the app, the CLI and the project files give them.
For the files themselves see `docs/project-format.md`; for every command see `docs/cli/`.

A short session that touches several of these terms (a terminal strip `-X1` in the location
`+CAB1`, a jumper, and a revision cloud):

```
wiringcraft new demo --no-git
cd demo
wiringcraft lib add std folder /path/to/wiringcraft/library
wiringcraft page add p0002 --number 2 --title Power
wiringcraft place p0002 wc.terminal --at 50,60 --tag +CAB1-X1.1   # prints s1
wiringcraft place p0002 wc.terminal --at 60,60 --tag +CAB1-X1.2   # prints s2
wiringcraft jumper add +CAB1-X1 1 2
wiringcraft location set +CAB1 "Main cabinet"
wiringcraft revision add A --text "Add -X1"
wiringcraft revision cloud p0002 40 50 70 70 --rev A
```

## Terms

**Accessory.** A further part of a device besides its main part, such as an auxiliary contact
block, a marker or an end plate, with a quantity. It is counted in the parts list.

**Base project.** A project that `wiringcraft new --template <folder or URL>` starts from. Its
pages, settings, title block, library sources and pictures are copied, not its comments or
history. Not the same as the page template.

**Black box (device box).** A rectangle with connection points on its edges, for a device that
has no symbol of its own. Wires attach to its points like pins. It is stored as a symbol of the
project (`box.<page>.<id>`), so it carries a tag and a part. Drawn with `box`, changed with
`box-edit`.

**Branch.** A line of work in the project's git history. `vcs branch` lists or creates one,
`vcs switch` changes to it and `vcs merge` brings another branch in. Because every page is its
own file, two people editing different pages on different branches do not conflict.

**Cable core.** A wire that runs in a cable. The wire names the cable device (for example `-W1`)
and the core (a number or a colour such as `BN`). A cable is an ordinary device; its part says
the type.

**Check.** `wiringcraft check` (and the live check in the app) looks for mistakes such as open
pins, wires ending in the air, shorted potentials and devices without a part, then runs the
house rules and the project's own rules. It exits with an error when it finds one.

**Check rule.** A rule `check` runs besides the built-in checks: a `[[rule]]` table in
`project.toml`, or a house rule in a library source's `rules.toml`. The project can turn house
rules off one by one (`rules_off`). See `wiringcraft rules`.

**Cloud (revision cloud).** A scalloped outline drawn around a change on a page, with a small
triangle that carries the revision letter. `revision cloud` draws one, `revision uncloud` takes
it off. A cloud must name a revision the project lists.

**Comment (review comment).** A note pinned to a spot on a page, with replies, open until
someone resolves it. Kept in `comments/<page id>.comments.json`. See `wiringcraft comment`.

**Cover page.** The one page marked as the project's cover (`page cover`). Its drawing area
shows the project's name and title-block fields, so editing them once updates the cover and
every title block. A new project starts with one.

**Cross-reference.** The page and column reference shown between symbols with the same tag,
for example from a relay coil to its contacts. It uses the page template's reference columns.

**Device.** One real-world product, drawn with one or more symbols on one or more pages that
share its tag. Its data (part, accessories, description, properties) is kept once, in
`devices.json`, keyed by tag.

**Device tag (tag).** A device's reference designation in the IEC 81346 style,
`=FUNCTION+LOCATION-PRODUCT`, for example `=PUMPS.P1+CAB1.R2-K3`. Each aspect is optional and
can have several levels joined by `.`. The product aspect (`-K3`) is what most people call the
tag.

**Function.** The `=` aspect of a device tag: what the device is for, such as a plant section
or a machine (`=PUMPS`). Unlike locations, functions have no list of their own; they exist in
the tags that use them.

**Interruption point.** A symbol where a wire stops and continues at every other interruption
point with the same tag, usually on another page. `check` reports one without a partner.

**Jumper (bridge).** Two or more terminals of one strip bridged together, so they form one
potential. A jumper can have a part (for example a WAGO 2-way jumper) that is counted in the
parts list. See `wiringcraft jumper`.

**Library lock (`library.lock`).** The exact version (source and content hash) of every library
item the project uses. `lib status` compares it with the sources and `lib update` takes newer
versions.

**Library source.** A folder or git repository of symbols and parts that a project draws from
(`lib add`). Earlier sources win over later ones. The items a project uses are copied into its
`vendor/` folder, so it opens the same without the source.

**Location.** The `+` aspect of a device tag: where the device is, such as a cabinet or a room
(`+CAB1`, or `+L1.L3` with levels). The project keeps a list of locations with descriptions
(`wiringcraft location`); removing one from the list leaves the devices' tags as they are.

**Macro.** A saved group of symbols, wires and drawings, one JSON file each in the `macros/`
folder of a project or a library source. Inserting one works like a paste: symbols get fresh
ids and the next free tags.

**Net.** The set of pins that are electrically connected, across all pages. `wiringcraft nets`
lists them; a net normally carries zero or one potential name.

**Overview page.** A page marked as an overview (line diagram) with `page overview`. It shows
devices again that schematic pages draw in detail, so its symbols do not count as the device's
drawings in `check`.

**Page.** One schematic sheet, stored as its own file `pages/<id>.page.json`. Its id (such as
`p0002`) is stable and is what commands take; its number (such as `2` or `12.3`) is what is
displayed and can change freely.

**Page template.** The layout every page is drawn on: the frame's reference columns and the
title block's boxes, kept under `[template]` in `project.toml`. A project without one uses the
built-in layout. See `wiringcraft template`.

**Panel layout.** A cabinet's mounting plate with its DIN rails, wire ducts and device
footprints, one file each in `panels/`. Layouts print after the schematic pages in the PDF.

**Part.** An orderable article (manufacturer, order number, description and technical data)
from a library source. A device's main part and its accessories are parts.

**Placeholder and value set.** A placeholder is a value a macro leaves open, written
`{{name}}` in its texts. A value set is one named variant that fills the placeholders, such as
the 1.5 kW size of a motor starter; placing the macro asks which one to use.

**PLC rack and card.** A rack is a PLC with I/O cards in its slots (`[[rack]]` in
`project.toml`), laid out by the PLC card overview report. See `wiringcraft rack`.

**Potential.** A signal name carried by a wire, such as `L1` or `24V`. Wires with the same
potential name are connected across pages, and the `potential` wire numbering rule uses it.
`check` reports a short when two potential names meet in one net.

**Revision.** One issued revision of the project: a letter or name unique in the project (`A`,
`B`, `01`), a date, what changed and who made it, kept in `project.toml`. See
`wiringcraft revision`.

**Revision table.** A table of the project's revisions shown on a page at a chosen spot
(`revision table`).

**Spare terminal.** A terminal that is on the rail but not drawn on the schematic, such as
`S1`. Added with `strip spare`; it shows in the terminal plan and the WAGO configurator export.

**Symbol.** A reusable drawing of a device function with named connection points (pins), from
a library source, for example `wc.terminal` or `iec.contactor.coil`. A placed symbol on a page
has its own id (`s1`, `s2`, ...) that wires refer to.

**Terminal level.** The level of a terminal on a multi-level terminal block, 1 being the
bottom. Set with `strip level`.

**Terminal strip.** A device whose terminals are tagged as one extra level of the strip's tag:
`-X1.1`, `-X1.2` are terminals 1 and 2 of strip `-X1`. The terminal strip editor
(`wiringcraft strip`) sets their order on the rail, spare terminals and terminal levels.

**Title block.** The band at the bottom of every page with the project's and the page's
fields (customer, revision, drawn by, ...). A page's own fields win over the project's on that
page (`page fields`).

**Translation dictionary.** The project's `translations.toml`: each text with its translation
into other languages. `wiringcraft translations` lists the texts that still lack one.

**Underlay.** A JPEG or PNG picture drawn under everything else on a page, such as a scanned
drawing to trace over. Kept in the project's `underlays/` folder.

**Wire number.** The number a wire carries in the wire list. Every wire of one connection gets
the same number; `number-wires` sets them by rule (`sequential`, `page-column` or `potential`).
