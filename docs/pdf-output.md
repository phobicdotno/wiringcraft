# What the PDF contains

`wiringcraft pdf` and Export, PDF in the app write the same kind of file: a vector PDF 1.7
written by WiringCraft itself. Lines and arcs stay vectors and all text is real text, so tags
and wire numbers can be searched and copied. The command's options (`--page`, `--date`,
`--report`, `--lang`) are described in the
[command-line reference for reports and exports](cli/reports-and-export.md); this page says
what comes out.

Export, PDF in the app prints every report that has rows, the panel layouts and the network
topologies, with no date of its own; drawn PLC card sheets come only from the command line.

## Order of the sheets

The sheets always come in this order:

| Sheet | Page number | When it is there |
|---|---|---|
| Table of contents | `R1` (more `R` numbers if it runs onto more sheets) | `--report contents` (or `all`) |
| Schematic pages, the cover page among them | the page's own number | always (every page, or the ones named with `--page`) |
| Panel layouts, one sheet each | `P1`, `P2`, ... | the project has panel layouts |
| PLC card sheets, one per card (more for a long card) | `C1`, `C2`, ... | `--report plc-cards` |
| The other report tables, in the order asked for | `R2`, `R3`, ... | `--report` |
| Network topologies, one sheet per network | `N1`, `N2`, ... | `--report networks` |

The schematic pages keep the project's page order. The cover page is a schematic page marked
as the cover (`wiringcraft new` makes page 1 the cover), so it is usually the first of them,
after the table of contents.

A report table that does not fit on one sheet carries on over as many sheets as it needs;
each gets its own `R` number and its bookmark shows `(1/3)`, `(2/3)` and so on. `all` leaves
out the PLC card overview in a project without racks and the network list in a project
without networks, so it does not add empty sheets; naming the report prints it anyway.

### Report pages

A report can also sit among the pages, as in EPLAN: make a page a report page with
`wiringcraft page report <page> parts` (any report name `--report` takes; `--off` makes it a
normal page again), or in the app with Page properties, Page type, Report: ... . The PDF prints that report there, in the page's place, with the page's
number and title on each of its sheets (the bookmarks show `(1/2)`, `(2/2)` for a long one),
and the page's own title-block fields. That report is then not printed again after the pages,
whatever `--report` or the project's report choice says. A report page is printed even when
its report is not chosen, since it is one of the pages; a PDF made with `--page` that leaves
the report page out leaves its report out too. The sheets use the report sheet size, not the
page's own. What is drawn on a report page is not printed, with a warning. The cover page
cannot be a report page, and a page with symbols or wires is refused (move them first).
A report page holding the table of contents lists the sheets after the pages as the front one
does. Topology pages and PLC card sheets still follow the pages when their report is on a
report page.

The table of contents lists every schematic page and then every sheet after them (panel
layouts, PLC cards, the other reports, networks) with its number. With `--page`, it lists only
the pages in the file.

The Sheet field counts every sheet in the file, reports included: `3 / 12`.

## Page sizes

- Schematic pages keep the size each page has (A4 to A0, landscape or portrait, or a custom
  size from 100 to 2000 mm a side; A3 landscape by default).
- Report tables and panel layouts use the size of the first schematic page in the file. A
  report page moves to the smallest ISO size with room for its title block and a few table
  lines when the first page is too small.
- PLC card sheets also use the first page's size, or the smallest ISO size that is wide and
  tall enough for a card row.
- A network topology gets the first page's size if the whole drawing fits, else the first ISO
  size that fits, else a custom sheet. A topology larger than 2000 mm is drawn on the largest
  sheet with a warning. [Fieldbus networks and topology pages](networks.md) says how it is
  drawn.

## Frame and title block

Every sheet, report and topology sheets included, has the same frame:

- a heavy border 10 mm in from the paper edge;
- numbered reference columns along the top (10 unless the page template sets 1 to 40);
- reference row letters down both sides, outside the border: A to F on sheets up to 297 mm
  high, more on taller sheets;
- the title block, a band across the bottom (24 mm high unless the template sets 10 to 60 mm),
  divided into the boxes of the page template (`wiringcraft template`).

The built-in title block has these boxes and values:

| Box | Value | Comes from |
|---|---|---|
| Customer | customer | `customer` in `[title_block]` of `project.toml` |
| Drawn by | drawn_by | title-block field `drawn_by` |
| Checked by | checked_by | title-block field `checked_by` |
| Project | project name, page title, project number and plant | the project's `name`, the page's title, fields `project_number` and `plant` |
| Revision | revision | title-block field `revision` |
| Date | date | title-block field `date`, else the `--date` text |
| Page | page number | the page's number (`2`, `R1`, `P1`, ...) |
| Location | location | title-block field `location` |
| Sheet | sheet | the sheet's place in the file and the number of sheets |
| (logo box) | logo | the project logo (`wiringcraft logo`), or the word WiringCraft when there is none |

A page can have its own value for any title-block field (usually `revision`, `date`,
`drawn_by` and `checked_by`); a page's own value wins over the project's. A blank date shows
the `--date` text, so the date is never guessed. The `revision` field is plain text: it is not
taken from the revision list. A value too long for its box is drawn smaller to fit; text is
never wrapped. A template can put any other field key in a box.

The cover page shows, above its title block, the project name in large type, the
`description` field under it, and a table of the standard fields (Customer, Project number,
Plant, Location, Drawn by, Checked by, Revision, Date) followed by any other field that has a
value. The logo, when there is one, goes above the name unless it has been moved.

## Cross-references

Cross-references are printed in blue on the schematic pages, as `page/column.row`, for example
`3/2.D`:

- A coil lists each of its contacts with the contact's pins: `13-14 3/3.B`.
- A contact points back to its coil under its tag: `> 3/2.D`.
- An interruption point lists where the wire carries on: `2/6.B` (several places are
  separated by spaces).

The column is the reference column the symbol sits in and the row is the row letter. Each
cross-reference is a link: clicking it opens the page it points to. A link to a page that is
not in the file (left out with `--page`) is not written.

Other links in the file:

- each row of the table of contents opens its sheet;
- rows of the wire list, cable list, terminal plan and PLC I/O list open the page the row
  describes;
- each channel row on a PLC card sheet opens the page its point is drawn on;
- each device box on a network topology opens the first page in the file where the device is
  drawn.

## Bookmarks, page labels and layers

- Every sheet has a bookmark (outline entry) with its number and title, such as
  `2 Main circuit` or `R2 Parts list`, and the reader opens with the bookmarks shown.
- The reader's page box shows each sheet's own number (`R1`, `2`, `P1`) instead of 1, 2, 3.
- The document title is the project name, and readers show it instead of the file name. The
  document's Author is the `drawn_by` field and its Subject the `description` field, when set.
- Some content is on layers (optional content) the reader can hide: Drawings and notes (free
  texts and graphics, revision clouds and the revision table), Wire numbers, Cross-references
  and Scanned sheets (underlay pictures). Only layers that are used are listed.

## Fonts and characters

All text uses Helvetica, one of the standard PDF fonts, with WinAnsi encoding. The font is not
embedded: every PDF reader has it. WinAnsi covers Western European letters (`æ ø å ä ö ü é`),
the euro sign, curly quotes and dashes. An arrow prints as `>`. Any other character (such as
`Ω`) prints as `?`, and the export warns with the page number and the characters. Bookmarks,
layer names and the document title can hold any character.

Pictures are embedded: a page's underlay with its page, and the logo once, however many
sheets show it.

## Revision clouds and the revision table

A revision cloud (`wiringcraft revision cloud`) is drawn on its page as a scalloped outline
with a tag holding its revision letter. A revision table (`wiringcraft revision table`) is
drawn where it was placed, with the columns Rev, Date, Description and By and one row per
revision in the project, oldest first. Both are on the Drawings and notes layer. The export
warns about a cloud with no area, a cloud whose revision the project does not list, and a
revision letter the font cannot print.

## Languages

With `--lang nb` the PDF prints the project's texts from
`translations.toml` in that language; with two languages (`--lang nb,en`) each text is shown as
`Hovedbryter / Main switch`. The texts translated are page titles, free texts on pages, and
device and location descriptions. Tags, wire numbers, part numbers, title-block fields and the
headings of report tables stay as they are. A text with no translation is printed as drawn and
listed as a warning. `wiringcraft translations` lists or sets the translations.

## Warnings

The export does not stop for problems in the drawing. It prints a warning for each one (a
symbol missing from the library copy, a contact without a coil, an interruption point without
a partner, a missing underlay picture, text the font cannot print, a cover too small for its
field table) and still writes the file. A project with no sheet to print writes nothing and
stops with `nothing to export: the project has no pages`.
