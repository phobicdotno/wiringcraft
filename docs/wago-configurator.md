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
| Jumper | each jumper of the strip | Connects (the terminals it joins); Article, Manufacturer and Description from the jumper's part (`wiringcraft jumper part`, or the terminal's properties in the app), else Description `3-way jumper` |
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
jumper, `/P<n>` for a plate and `/R<n>` for a rail, e.g. `-X1/T3`. An accessory row names the
device slot it is: `<strip>/S` for the strip device's own part, `<strip>/A<n>` for the strip
device's n-th accessory, and `<strip>/T<position>/A<n>` for the n-th accessory of the terminal
device first at that position (`-X1/T2/A1`). It stays the same as long as the strip does not
change (adding an accessory to one device leaves the other rows' indexes as they are), so a list
that comes back from the configurator with articles chosen can be matched to the project's
devices row by row.

## Import back

`wiringcraft import wago <file>` (app: File → WAGO articles back; ribbon: Start → Project) reads
the list after the configurator has filled in the articles, and writes them onto the project's
devices and jumpers:

- The header must have the `Index` and `Article` columns; `Type`, `Strip`, `Terminal`,
  `Connects`, `Manufacturer` and `Quantity` are read when present. The leading `'` the export puts on some
  cells is dropped. Rows without an article are passed over.
- Each article is looked up by part id (as the export writes a part without a copy) or by order
  number (ignoring case), among the project's own parts first (parts made for the project, not
  taken from a library); the libraries' parts, and those the project took from them, count only
  when none of the project's own match. When several parts have the order number, the row's
  Manufacturer chooses. A part the project locks but whose copy cannot be read is never given to
  a device. A library part that is used is taken from the library it was found in
  into the project and locked, as when a part is chosen by hand.
- A Terminal row's article becomes the part of the terminal device (`-X1.3`, or the strip device
  for terminals tagged only with the strip). An accessory row's article and quantity replace that
  accessory; `<strip>/S` is the strip device's own part.
- A Jumper row's article becomes the part of the strip's jumper with that number
  (`-X1/J2` is the second jumper of `-X1`, in the order the export writes them), as with
  `wiringcraft jumper part`. The row's Connects must still name the jumper's terminals (spaces
  do not matter); otherwise the jumpers changed since the export and the row is not taken.
- A row the configurator added without an index, with a Strip the project has, becomes a new
  accessory of the strip device (unless it has that part already), with its quantity. A row
  without an index that is a Jumper, or has no Type but names terminals in Connects, is not taken:
  it may be a jumper that lost its index.
- In the app it is one undo step.
- A copy the project has of a part without its pin (left by an undone import) is matched as it
  is and pinned again, never replaced by a newer library version.
- The command line import also pins every part the project already uses without a pin (left
  by an earlier import whose lock could not be written), even one no row names: then only
  `library.lock` is written (and the part's copy, when the project has none). With every used
  part pinned and nothing to change, nothing is written.

Every row with an article that is not taken is listed with the reason: an index the project no
longer has, or whose type, terminal number or (for an accessory) terminal in Connects changed
(export again), a jumper whose terminals in Connects changed (export again), a terminal's
accessory or a jumper in a file without the Connects column, a plate or rail, an article no part has (import the maker's catalogue into a library
first, e.g. `import parts-bmecat`), an article several parts have, or two rows that give one
device part different articles or quantities (neither is taken, and both are listed). Importing the same list twice changes nothing the
second time.

## Limits

- WAGO's own exchange format between its EPLAN interface and the configurator is not publicly
  documented, so this list uses WiringCraft's own column layout. Map the Article and Quantity
  columns in the configurator's article-list (Excel/CSV) import; the other columns say where
  each item goes. This layout has not been checked against a live configurator yet.
- Rails in WiringCraft's panel layouts are horizontal, so the rotation is always `0`.
- Strip tags such as `-X1` start with `-`; a spreadsheet that opens the file may take them for
  formulas. The WAGO tools read them as text, as in the marker export, so such tags are written
  as they are. A tag with a function or location aspect (`=A-X1`, `+C1-X1`) gets a leading `'`
  in the Index and Strip columns, with a warning. Free texts (article, manufacturer, description, wire specs, plate name)
  that start with `=`, `+`, `-` or `@` are written with a leading `'` so a spreadsheet shows them
  as text, and the export warns about each one.
