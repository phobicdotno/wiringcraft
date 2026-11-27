# Parts

A part is something you can order: one article of one maker, with its order number. A device
on the drawing (`-K1`) is given a part, and the parts list (bill of materials) counts the parts
of all devices. This page explains how that fits together.

For the files a library holds, see [library-format.md](library-format.md). For where makers
publish part data and which formats WiringCraft reads, see
[manufacturer-data.md](manufacturer-data.md).

## What a part holds

A part lives in a library source as one JSON file under `parts/`. It has:

- an id, usually `<maker>.<order number>` in lower case with other characters turned into `-`
  (`Acme` and `AB-123` give `acme.ab-123`),
- the maker, the order number and a description,
- free properties, such as `Rated current` or `Coil voltage`,
- optionally: the symbol it is usually drawn with, the contacts it has (main, NO, NC,
  changeover), an outline drawing for cabinet layouts (`import footprint-dxf`) and the size of
  its 3D model (`import model-step`).

The repository's own `library/parts/` holds example parts (maker `Example`) to try things with.
They are not real articles.

## Devices and parts

The part belongs to the device, not to one symbol. A contactor drawn as a coil on page 2 and
contacts on page 5 is one device, `-K1`, with one part. Every symbol carrying the tag shows the
same part data.

A device has:

- a main part (or none),
- accessories: more parts that belong to it, each with a quantity (an auxiliary contact block,
  a marker, an end plate),
- a description and its own properties.

This is stored in the project's `devices.json`.

On the command line, `device` sets or shows the main part and the description:

```text
$ wiringcraft device -K1 --part example.ex-ct-9-24dc --description "Pump contactor"
$ wiringcraft device -K1
part         example.ex-ct-9-24dc
description  Pump contactor
```

`--part ""` takes the part off again. A part id that no library source has is refused
(`Part "nope" is not in any library source`). `wiringcraft lib search <words>` finds part ids.

In the app, select a symbol: the properties panel has a **Part** section with a searchable
picker, the part's maker, order number, description and properties, and an **Accessories**
section where parts are added and their quantity is changed with `+` and the minus button.
Accessories are set in the app (or by `import wago`); the `device` command sets the main part
only.

## The project's copy

When a device is given a part from a library, the project takes a copy of it into
`vendor/parts/<id>.json` and pins its version in `library.lock`. Reports, the PDF and the
check read that copy, not the library. So a change in the library does not change a finished
drawing by itself: `wiringcraft lib status` shows which parts changed, and `wiringcraft lib
update` takes the new versions. See [versions.md](versions.md).

The exception is `import footprint-dxf` and `import model-step`: when the project uses the part
and takes it from the library source the import writes to, they update the project's pinned
copy too (unless that copy has local changes, which a warning names). The device's size on a
cabinet layout can then change without `lib update`.

## The parts list (bill of materials)

`wiringcraft report parts` (and `pdf --report parts`) lists one row per part with its quantity,
maker, order number, description and the devices that use it:

```text
$ wiringcraft report parts
Parts list
Pos  Qty  Manufacturer  Order number  Description                                                        Devices
---  ---  ------------  ------------  -----------------------------------------------------------------  -------
1    1    Example       EX-CT-9-24DC  Contactor 3-pole 9 A, 1 NO auxiliary, 24 V DC coil (example data)  -K1
     1                                no part assigned                                                   -K2
```

How the quantity is counted:

- a device's main part counts once, its accessories with their quantity,
- a terminal strip device whose terminals are drawn tagged with only the strip (`-X2`) counts
  its part once per terminal number,
- a jumper with a part counts once, listed under its strip,
- devices without any part come last, as `no part assigned`, so the gaps are easy to see,
- a part the project's copy does not have shows its id as the order number and the text
  `part not in the project's library copy`.

The same report can be written as CSV, Markdown or Excel (`--format`). See
[reports.md](reports.md).

## What the check does with parts

`wiringcraft check` looks at parts in three ways:

- a device whose part (or accessory) is missing from the project's library copy is a warning,
- a device that draws more contacts than its parts have is a warning on each of its contact
  symbols, for example `-K1 draws 3 main, 2 NO contacts, its parts have 3 main, 1 NO (1 NO too
  many)`. The contacts of the main part and of the accessories (times their quantity) add up.
  Only parts that list their contacts are checked,
- a device without a part is a note, shown with `check --all`.

Cabinet layouts take a device's size from its part: the `Width` and `Height` properties first,
then the outline drawing, then the 3D model.

## Importing catalog parts

Parts from makers' catalogs come in through the import commands. Each writes part files into
a library folder given with `--lib`:

- `import parts-csv`: a CSV, semicolon-separated or Excel (`.xlsx`) parts list. Columns are
  found by name in several languages (`Manufacturer`, `Order number`, `Description`,
  `Symbol`, contact counts); every other column becomes a property,
- `import parts-bmecat`: a BMEcat catalogue (XML 1.2 or 2005),
- `import parts-edz`: an EPLAN Data Exchange archive, with symbols made from its macros,
- `import swe-library`: the catalog parts of a SolidWorks Electrical archive.

What each format gives is listed in [manufacturer-data.md](manufacturer-data.md).

On the command line the folder is not added to the project's library sources for you. Add it
once, then give devices its parts:

```text
$ wiringcraft import parts-csv parts.csv --lib mylib
imported 1 part(s) into mylib
$ wiringcraft lib add mine folder mylib
$ wiringcraft device -K2 --part acme.ab-123
```

In the app, the parts imports in the **File** menu (EDZ, BMEcat and CSV) ask for the library
folder after the file and add it to the project's libraries once parts are in it. The
SolidWorks Electrical libraries import is different: the app puts its parts, cable types and
symbols into the project itself and asks for no folder. To write them into a library folder,
use `import swe-library --lib` on the command line.

Importing again writes the part files again. A project that already uses one of those parts
keeps its own copy until `lib update` takes the new version, as above.
