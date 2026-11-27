# WAGO smartScript marker export

WiringCraft writes marker lists that WAGO smartScript reads through its Excel/CSV import. The
app has it under File → WAGO marking (ribbon: Start → Project → WAGO marking); the command line
has `wiringcraft export wago-marking <out>`.

The file is semicolon-separated UTF-8 with a byte-order mark and CRLF line ends, so Excel and
smartScript read it in European locales without garbling `æøå`. There is one row per marker,
sorted in natural tag order (`-X2` before `-X10`). On a terminal strip arranged in the
terminal strip editor, its markers come in rail order, spares included, so they print in the
order they go on the rail.

## Columns

| Column | What it holds | Map it in smartScript to |
|---|---|---|
| Type | Terminal, Wire, Device or Cable (in the chosen language) | nothing, or a filter |
| Group | the terminal strip, or a device's or cable's location (`+C1`) | nothing, or a filter |
| Marker | the marker text (with one line) | the marker text |
| Line 1, Line 2, ... | one column per text line (with more than one line) | the marker's lines, in order |
| Count | copies to print: 2 for a wire number or a cable (one at each end), 1 otherwise | the quantity |

smartScript asks which column goes to which marker field when the file is imported. Only the
marker text (or the line columns) and the count need mapping; Type and Group help sort and filter.

## Options

| Option | App | Command line | Effect |
|---|---|---|---|
| Marker kinds | Markers checkboxes | `--kind terminals,wires,devices,cables` | which markers are listed; default all |
| Text lines | Line 1 to Line 3 | `--line marker,description` | one column per line; default one marker line |
| Language | Language | `--lang en\|de\|nb` | the header row and the Type column |
| One terminal strip | Terminal strip, or a terminal's menu in the Devices list | `--strip -X1` | limits the terminals to that strip; other kinds chosen as well (such as wires) are listed in full |
| Level separator | Level separator | `--level-separator " / "` | written between the numbers of a multi-level terminal's levels; default ` / ` |

A line shows one of:

- `marker`: the marker (engraving) text. For a device or cable, its `Marker` property when it is
  set (a customer's own name, for example), otherwise its tag. A terminal's number, a wire's number.
- `tag`: the tag, ignoring the `Marker` property. A terminal shows its strip.
- `description`: the device's description; a terminal shows its strip's description. Empty for wires.
- `location`: the location part of the tag (`+C1`). Empty for wires.

With a strip and no kinds, only that strip's terminals are written. With a strip and kinds, the
strip limits the terminals and the other kinds are listed in full. An unknown kind, line,
language or strip is refused before anything is written.

## Multi-level terminals

A double- or triple-deck terminal gets one marker, not one per level. A terminal's level is its
`Level` device property (`1` at the bottom, `2` above it, ...): a terminal with level 2 or more
sits on the same block as the terminal before it in the strip. So with `-X1.2` at level 2,
`-X1.1` and `-X1.2` print as one marker `1 / 2`. The bottom level needs no property of its own.
The terminal plan lists each level in its Level column. A `Level` that is not a whole number
from 1 up is ignored, and the export warns about it with the device, the value and the reason.

The app asks where to save the list, starting at `export/wago-marking.csv`, or
`export/wago-marking-<strip>.csv` for one strip. When the save dialog cannot open (Linux without a
desktop portal or zenity) it writes there directly and says so.

## Not written

WAGO's EPLAN add-in also uses template files for smartScript's MultiImport. Their layout is not
publicly documented, so they are not written; the CSV import above covers the same print jobs.
A marker text that starts with `=`, `+`, `-` or `@` is written as it is (smartScript prints it),
with a warning, since a spreadsheet opening the file may take it for a formula. Every line column
is checked: wire numbers always warn, and a device's `Marker` text or description warns unless it
is written exactly like a tag (`-K1`, `=A+C1-K1`), which these markers are by design.
