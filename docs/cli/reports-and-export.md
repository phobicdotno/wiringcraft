# Reports and export on the command line

This page covers the commands that read a project and write something out: `report`, `nets`,
`pdf`, `drawing`, `aml`, `export` and `report-layout`. Each one works on the project in the
current folder, or on the one given with `-C <dir>` (`--project <dir>`). Run
`wiringcraft <command> --help` for the full option text.

The examples use the motor-starter project that `examples/demo.sh` builds: pages `p0001` (cover),
`p0002` and `p0003`, devices such as `=PUMP+CAB1-K1`, terminal strip `=PUMP+CAB1-X1` and cable `-W1`.

## report

```
wiringcraft report <KIND> [--format text|csv|md|xlsx] [--out <file>] [--sep ,|;]
```

`KIND` is one of these, or `all` for every report:

| Kind | What it lists |
|---|---|
| `contents` | the pages: number, title, how many devices and wires |
| `parts` | the parts list: position, quantity, manufacturer, order number, description, devices |
| `devices` | every device: description, part, symbols, pages |
| `wires` | every wire: page, number, potential, cable, from, to, section, colour, type |
| `nets` | every connection (potential) and the pins on it |
| `terminals` | the terminal plan: strip, terminal, what is on each side, potential, jumpers, level |
| `cables` | every cable core: cable, type, description, core, page, from, to |
| `plc` | the PLC I/O list: address, type, device, what it is connected to |
| `plc-cards` | the PLC cards by rack and slot, channel by channel (see `wiringcraft rack`) |
| `locations` | the locations (`+` part of the tags) and how many devices each holds |
| `networks` | the network ports of devices: network, address, station, port, cable |

`--format` picks the output:

- `text` (the default): a table under a title line, for reading in a terminal.
- `csv`: comma-separated, or semicolon-separated with `--sep ';'`. With `;` and `--out`, the
  file starts with a UTF-8 byte order mark, so Excel in a comma-decimal locale opens it in
  columns. A cell that starts with `=`, `+`, `@` or a `-` that is not a number (a tag such as
  `=PUMP+CAB1-K1` or `-W1`) gets a leading `'`, so a spreadsheet shows it as text and does
  not run it as a formula.
- `md`: a Markdown heading and table.
- `xlsx`: an Excel workbook, one sheet per report. It needs `--out`.

With `all`, the reports follow one another on standard output. With `all --format csv --out
<folder>`, the folder gets one `<report>.csv` per report.

```
wiringcraft report parts
wiringcraft report terminals --format md
wiringcraft report wires --format csv --sep ';' --out wires.csv
wiringcraft report all --format xlsx --out reports.xlsx
wiringcraft report all --format csv --out reports
# wrote 11 csv file(s) to reports
```

An unknown kind, format or separator is an error and nothing is written. A report with a layout
of its own (see `report-layout` below) shows its title, columns, sorting and grouping in every
format.

## nets

```
wiringcraft nets
```

Prints every potential across all pages, one per line: its name, then its pins as
`<device tag>:<pin>`. A potential is named after the wire potential set on it (`L1`, `24V`),
otherwise `net<N>`. Pins on a symbol without a tag show the symbol's item id (`s4:PE`).
Interruption points join the potentials on both pages.

```
$ wiringcraft nets
L1: =PUMP+CAB1-Q1:1
net2: =PUMP+CAB1-Q1:2 =PUMP+CAB1-K1:1
...
24V: -24V:1 =PUMP+CAB1-S1:11 =PUMP+CAB1-K1:13 -24V:1
```

`wiringcraft report nets` lists the connections as a table (net, pin count, pins) and takes the
report formats.

## pdf

```
wiringcraft pdf <OUT> [--page <id>]... [--date <text>] [--report <kind>]... [--lang <lang>]
```

Writes the schematic pages to one PDF, followed by the panel layouts (`wiringcraft panel`).

- `--page`: only these pages, by id. Repeat it for more than one page. Panel layouts are still
  added.
- `--date`: the date for the title block where neither the page nor the project sets one. It is
  printed as given.
- `--report`: append report pages. Repeat it for more than one report, or give `all`. The kinds
  are those of `report`. Without `--report` the project's own choice is printed (see
  `pdf-reports` below), if it has one. `--report none` prints no report.
- `--lang`: print the project's texts in a language from `translations.toml` (`nb`), or in two
  (`nb,en`, each text with the second language after it). Texts without a translation are
  printed as drawn and listed as warnings.

```
wiringcraft pdf pump-station.pdf --date 2026-11-27 --report all
wiringcraft pdf control.pdf --page p0003
wiringcraft pdf lists.pdf --page p0002 --page p0003 --report parts --report terminals
wiringcraft pdf pump-nb.pdf --lang nb
```

## pdf-reports

```
wiringcraft pdf-reports [<kind>...] [--clear]
```

The report pages `pdf` prints when no `--report` is given, kept as `pdf_reports` in
`project.toml`. File → Export PDF in the app prints the same choice, and Project properties sets
it there under "Reports in the PDF".

- No argument: print the choice, or say that there is none.
- Kinds (those of `report`, `all`, or `none` for no report): replace the choice. Each is checked
  first; a wrong name changes nothing.
- `--clear`: remove the choice, so `pdf` prints only the reports it is asked for and the app
  prints every report that has rows.

```
wiringcraft pdf-reports contents parts terminals
wiringcraft pdf drawings.pdf
# the same as --report contents --report parts --report terminals
```

## drawing

```
wiringcraft drawing <OUT-FOLDER> [--format svg|dxf] [--page <id>]... [--date <text>] [--lang <lang>]
```

Writes one file per page into a folder, drawn as in the PDF (frame, title block, symbols, wires,
cross-references), and one file per panel layout. Pages are named `<page number>.<format>` and
panel layouts `panel-<name>.<format>`; characters that do not belong in a file name become `_`.
`--format` is `svg` (the default) or `dxf`. Only SVG shows pictures placed on pages.
`--date` and `--lang` work as for `pdf`. `--page` writes only the pages named, by id (repeat it
for more than one), and no panel layouts; without it every page and every panel layout is
written.

```
$ wiringcraft drawing svg
wrote 4 svg file(s) to svg
$ ls svg
1.svg  2.svg  3.svg  panel-Cabinet_1.svg
$ wiringcraft drawing dxf --format dxf --page p0003
wrote 1 dxf file(s) to dxf
```

## aml

```
wiringcraft aml <OUT>
```

Writes an AutomationML (CAEX 3.0) file for tools that take engineering data rather than drawings:
one element per device with its part data and connection points, and one link per connection.
Pins of symbols without a device tag are left out, with a warning that counts them.

```
$ wiringcraft aml pump.aml
warning: 1 connected pin(s) on symbols without a tag were left out
wrote pump.aml: 9 devices, 13 connections
```

## export

`export` writes files for other tools. Both subcommands write a semicolon-separated UTF-8 CSV.

### export wago-marking

```
wiringcraft export wago-marking <OUT> [--kind <kinds>] [--line <lines>] [--lang en|de|nb]
                                      [--strip <strip>] [--level-separator <text>]
```

A marker list for WAGO smartScript: one row per terminal, wire, device or cable marker. `--kind`
takes `terminals`, `wires`, `devices`, `cables` (default all), `--line` the text lines `marker`,
`tag`, `description`, `location`, and `--strip` one terminal strip, named in full as the
terminal plan names it. The columns and options are described in
[WAGO smartScript marker export](../wago-marking.md).

```
wiringcraft export wago-marking markers.csv
wiringcraft export wago-marking wires.csv --kind wires,cables
wiringcraft export wago-marking x1.csv --strip "=PUMP+CAB1-X1" --line marker,description --lang de
```

### export wago-configurator

```
wiringcraft export wago-configurator <OUT>
```

The terminal strips for the WAGO configurator (smartDESIGNER): for each strip its plates and DIN
rails, its terminals in strip order, its jumpers and accessories. It has no options. Cells that a
spreadsheet could run as a formula are written with a leading `'`, with a warning for each. See
[WAGO configurator export](../wago-configurator.md).

```
$ wiringcraft export wago-configurator strips.csv
...
wrote strips.csv (4 rows)
```

## report-layout

A report's layout is a `[[report]]` table in `project.toml`: the columns in their order and
with their headings, the sorting, the grouping, a title, and title-block values printed above the
table on the report's PDF pages. `report-layout` reads and writes these tables.

```
wiringcraft report-layout show [KIND]
wiringcraft report-layout set <KIND> <FILE | ->
wiringcraft report-layout reset <KIND>
```

`show` prints JSON for one report, or for all of them: its kind, its layout (`null` for the
built-in one), its built-in column headings and warnings about the layout. Name columns in a
layout by these built-in headings.

`set` replaces a report's layout with a JSON object read from a file, or from standard input
with `-`. The keys are:

- `title`: a title in place of the built-in one.
- `column`: the columns to show, in order, each `{"from": "<built-in heading>", "heading":
  "<new heading>"}`; `heading` is optional. Left out, the built-in columns show.
- `sort`: columns to sort by, most significant first; a `-` in front sorts that column
  descending.
- `group`: a column to group the rows by. In the PDF each group gets a heading and the column
  leaves the rows; a CSV keeps the value in every row.
- `header`: title-block keys printed above the table in the PDF, such as `project.name`,
  `revision` or `customer`.

An unknown key is refused. A column that does not exist is kept, with a warning.

```
$ cat parts-layout.json
{
  "title": "Bill of materials",
  "column": [
    {"from": "Order number", "heading": "Art. no."},
    {"from": "Qty"},
    {"from": "Description"}
  ],
  "sort": ["-Qty", "Order number"],
  "header": ["project.name"]
}
$ wiringcraft report-layout set parts parts-layout.json
the parts report's layout is set
$ echo '{"group": "Part"}' | wiringcraft report-layout set devices -
$ wiringcraft report-layout show parts
$ wiringcraft report-layout reset devices
the devices report has its built-in layout
```

`reset` removes the report's `[[report]]` table.
