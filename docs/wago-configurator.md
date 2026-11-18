# WAGO configurator export

WiringCraft writes the project's terminal strips as a list for the WAGO configurator
(smartDESIGNER). The app has it under File → WAGO configurator (ribbon: Start → Project → WAGO
configurator) and writes
`export/wago-configurator.csv`; the command line has `wiringcraft export wago-configurator <out>`.

The file is written the same way as the [smartScript marker export](wago-marking.md):
semicolon-separated UTF-8 with a byte-order mark and CRLF line ends, so Excel and the WAGO tools
read it in European locales without garbling `æøå`.

## Rows

Strips come in natural tag order (`-X2` before `-X10`). For each strip:

| Type | One row for | Filled in |
|---|---|---|
| Plate | each panel layout that holds the strip device or one of its terminals | Plate (the layout's name), Description (`Mounting plate 600 x 800 mm`) |
| Rail | each DIN rail of that layout that the strip's footprints sit on | Plate, Rail (its number on the plate, from 1, in the layout's order), Rail length (mm), Rail rotation |
| Terminal | each terminal, in strip order | Position, Terminal (its number), Level, Article, Manufacturer, Description, the wires' section and colour on each side |
| Jumper | each jumper of the strip | Connects (the terminals it joins), Description (`3-way jumper`) |
| Accessory | the strip device's own part and accessories (end plates, end stops), then each terminal device's accessories, once per device | Article, Manufacturer, Description, Quantity, Connects (the terminal, for a terminal's accessory) |

A terminal's article is the part of the device it is drawn as: `-X1.3`, or the strip device
itself for a terminal tagged only with its strip (`-X2`, numbered in drawing order). The strip
device's part then belongs to its terminals and is not listed again as an accessory. A part
without a vendored copy is written by its id (the command line also warns when a part file
cannot be read). Upper is the terminal's first pin, lower its second; a side with wires of different
sections or colours lists each, separated by commas. A multi-level terminal's levels are
separate rows with their level number, as in the terminal plan.

## Index

The first column identifies each row: `<strip>/T<position>` for a terminal, `/J<n>` for a
jumper, `/A<n>` for an accessory, `/P<n>` for a plate and `/R<n>` for a rail, e.g. `-X1/T3`. It
stays the same as long as the strip does not change, so a list that comes back from the
configurator with articles chosen can be matched to the project's devices row by row (#123).

## Limits

- WAGO's own exchange format between its EPLAN interface and the configurator is not publicly
  documented, so this list uses WiringCraft's own column layout. Map the Article and Quantity
  columns in the configurator's article-list (Excel/CSV) import; the other columns say where
  each item goes. This layout has not been checked against a live configurator yet.
- Rails in WiringCraft's panel layouts are horizontal, so the rotation is always `0`.
- Jumpers have no part of their own yet: their Article is empty.
- Strip tags such as `-X1` start with `-`; a spreadsheet that opens the file may take them for
  formulas. The WAGO tools read them as text, as in the marker export, so the tag columns are
  written as they are. Free texts (article, manufacturer, description, wire specs, plate name)
  that start with `=`, `+`, `-` or `@` are written with a leading `'` so a spreadsheet shows them
  as text, and the export warns about each one.
