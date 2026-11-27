# Cross-references

A device can be drawn in more than one place: a relay coil on one page and its contacts on
others, or a wire that leaves one page and carries on on another. WiringCraft links these
places by their device tag and prints a short reference next to each one, so a reader can
find the other end. The references are worked out from the drawing every time a page is
shown or printed; nothing about them is stored in the project.

## What takes part

What a placed symbol does in a cross-reference comes from the role of its library symbol.
The symbol editor shows it as Role:

| Role | Symbol editor label | Bundled symbols |
|---|---|---|
| `main` | Main (coil) | `wc.coil`, `wc.coil.ondelay`, `wc.coil.offdelay`, `wc.overload.3p` |
| `auxiliary` | Auxiliary (contact) | `wc.contact.no`, `wc.contact.nc`, `wc.contact.co`, the on-delay and off-delay contacts, `wc.contact.overload.nc`, `wc.contactor.main3` |
| `interruption` | Interruption point | `wc.interruption` |

Symbols with the role `standalone` or `terminal` get no cross-reference text.

A main symbol and its auxiliary symbols belong together when they carry the same device tag,
the whole tag (`=function+location-product`) and not just the `-K1` part. A symbol with no
tag takes no part.

## Coil and contacts

- Under a coil, one line per contact with the same tag, on any normal page: the contact's
  pin names joined by `-` and where it is, for example `13-14 3/3.B`.
- Under a contact's tag, a pointer back to its coil, for example `> 2/2.B`. If the tag is on
  more than one coil, the contact points to the first one in page order, and the project check
  reports the tag as drawn twice.
- When the device's part (or its accessories) lists how many contacts it has, the check warns
  if the contacts drawn with that tag on normal pages are more than that, for example a
  relay with two NO contacts drawn with three. See `contacts` in a part's file.
- A contact whose tag is on no coil gets no text. The PDF export and the project check both
  warn `contact -K2 has no coil`.

## Interruption points

An interruption point (an off-page connector) says that the wire goes on somewhere else.
Every interruption point with the same tag is one connection:

- Next to each point, the places of all the other points with that tag, separated by spaces,
  for example `3/1.B`.
- For the net list, wire numbers and the check, only points that have a wire on them join
  their wires together. An unwired point still gets its reference text.
- A point whose tag is on no other point gets no text, and the PDF export and the check warn
  `interruption point -L9 has no partner`. The check also warns about a point without a tag,
  as it joins nothing.

There is no separate potential arrow symbol. To join wires across pages without symbols, give
them the same potential name (`wire --potential`, or Potential in the wire's properties); the
wire may end at a free point. That joins the nets but makes no cross-reference text.

## Overview pages

A page marked as an overview (line diagram, `wiringcraft page overview` or `page add
--overview`) shows devices again on purpose:

- A contact on an overview page is not listed under its coil.
- A contact points to its coil on a normal page before a coil drawn again on an overview page.
- An interruption point on a normal page only counts partners on normal pages. One on an
  overview page counts partners on any page.

## How a place is written

A place is `page/column.row`:

- `page` is the page number as set on the page (`--number`), not its id.
- `column` is the reference column the symbol's origin falls in, counted from 1 at the left.
  The frame has 10 columns unless the project's template sets another count
  (`wiringcraft template columns`).
- `row` is the reference row as a letter, `A` at the top. The number of rows follows the sheet
  height: six (A to F) up to A3 landscape, and 8, 10 or 12 on taller sheets.

## In the app

The canvas draws the references in the wire label colour. Hovering one underlines it, and clicking
it with the Select tool opens the page it points at (the first one, when it lists several).
The Layers window can hide the cross-references or lock them so they cannot be clicked.

Placing a symbol in the app gives it the next free tag for its letter code, so a new contact
starts out as a device of its own (`-K2`). Type the coil's tag in its Device tag field in the
properties panel to make it one of that coil's contacts. The interruption point has no letter
code, so it starts without a tag. The check runs again shortly after each edit and marks a
contact without a coil or a point without a partner on the canvas.

## In the PDF and drawings

`wiringcraft pdf` prints the same texts. Each one is a link to the page it points at, and they
sit on a PDF layer called Cross-references that a reader can turn off. `wiringcraft drawing`
draws them in SVG and DXF too; in DXF they are on the layer `CROSS-REFERENCES`.

## Example

A coil `-K1` on page 2 with two of its contacts on page 3, and an interruption point `-L1` on
both pages:

```sh
wiringcraft page add p2 --number 2 --title Control
wiringcraft page add p3 --number 3 --title Motor
wiringcraft place p2 wc.coil --at 60,100 --tag -K1
wiringcraft place p3 wc.contact.no --at 120,60 --tag -K1
wiringcraft place p3 wc.contact.nc --at 200,150 --tag -K1
wiringcraft place p2 wc.interruption --at 300,80 --tag -L1
wiringcraft place p3 wc.interruption --at 40,80 --tag -L1
wiringcraft pdf out.pdf
```

On an A3 page this prints `13-14 3/3.B` and `11-12 3/5.D` under the coil, `> 2/2.B` under
each contact, `3/1.B` next to the point on page 2 and `2/8.B` next to the one on page 3.
`place` on the command line sets no tag by itself, so give `--tag` to every symbol that should
take part.

Related: [docs/cli/](cli/) for each command's options, and `wiringcraft check --help` for
everything the check looks at.
