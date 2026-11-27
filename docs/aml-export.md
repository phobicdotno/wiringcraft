# AutomationML export

The AutomationML export writes the project's devices, their part data and connection points,
and the connections between them as one AutomationML file (IEC 62714, CAEX 3.0). It is for
tools that take engineering data rather than drawings. Pages, graphics and wires as drawn are
not in it.

## Running it

From the command line:

```
$ wiringcraft aml plant.aml
warning: 2 connected pin(s) on symbols without a tag were left out
wrote plant.aml: 9 devices, 13 connections
```

The file path is taken from the current folder, not from the `-C` project folder. The folder it
goes in must exist. The command will not write over one of the project's own files (for example
`devices.json`).

In the app, File > AutomationML (also in the Project group of the Start ribbon tab) asks where to save, starting at
`export/<project name>.aml` in the project folder, and creates the `export` folder when needed.

## File structure

```
CAEXFile  FileName, SchemaVersion="3.0"
  SourceDocumentInformation  OriginName="WiringCraft", OriginVersion, LastWritingDateTime
  InstanceHierarchy  Name = project name
    InternalElement  Name = project name, ID="project"
      InternalElement  one per device (ID d1, d2, ...)
        Attribute ...
        ExternalInterface  one per connection point (ID d1.c1, d1.c2, ...)
      InternalLink ...  the connections
```

- `FileName` is the name of the file written, such as `plant.aml`.
- The project name is the `name` in `project.toml`. When it is empty, `Project` is used.
- `OriginVersion` is the WiringCraft version.
- `LastWritingDateTime`: the CLI writes the current UTC time. The app always writes
  `1970-01-01T00:00:00Z`, so exporting the same project twice gives the same file and it diffs
  cleanly in git.
- There are no role or interface class libraries. Every attribute has
  `AttributeDataType="xs:string"`.

## Devices

Each device becomes an `InternalElement` named by its full device tag (`=PUMP+CAB1-K1`). That
covers every device in the project's device list and every tagged symbol on any page, so a
device that is only in the list (a cable such as `-W1`, a spare) is written too, with no
connection points. Devices are sorted by tag in natural order (`-K2` before `-K10`).

The attributes, in this order:

| Attribute | Comes from |
| --- | --- |
| `ReferenceDesignation` | The device tag. |
| `Function text` | The device description, when it has one. |
| `Manufacturer` | The part's manufacturer, when the part is in the project's library copy and has one. |
| `OrderNumber` | The part's order number. When the part is not in the project's library copy, the part id instead. |
| `PartDescription` | The part's description, when it has one. |
| any other name | Each of the device's own properties, by name, sorted by name. A PLC point's `Address` goes along this way. |

A device property named like one of the first five attributes is not written again, so it never
overrides them.

## Connection points

Each device gets one `ExternalInterface` per pin name on any of its symbols, on any page,
sorted by name. The pins come from the symbols in the project's library copy (`vendor`). Two
symbols of one device with the same pin name share one interface: in the demo project the two
auxiliary contacts of `-K1` both have pins `13` and `14`, so both potentials end on the same
`13` and `14` interfaces.

## Connections

Connections are built per potential, the same potentials `wiringcraft nets` lists (wires,
interruption points with the same tag, and jumpers between terminals). The device pins of one
potential are sorted by `tag:pin` and joined as a chain: the first to the second, the second to
the third, and so on. Each `InternalLink` is named after the potential: its name when it has
one (`24V`), else the generated `N1`, `N2`, ... as in the reports. A potential with more than two
pins gives several links, named `N9`, `N9#2`, `N9#3`.

```
<InternalLink Name="N9" RefPartnerSideA="d2.c2" RefPartnerSideB="d5.c2"/>
<InternalLink Name="N9#2" RefPartnerSideA="d5.c2" RefPartnerSideB="d6.c1"/>
```

A potential with only one device pin gives no link.

## What is left out

- Interruption points. They only join potentials; a tag drawn only as interruption points is
  not a device.
- Symbols without a tag, and their pins. A link to such a pin is not written.
- Pages, positions, graphics, texts and the PDF reports.
- Wire numbers, colours, cross-sections, cables and their cores (a cable tag shows only as a
  device with no connection points), and terminal strip order.
- Accessory parts of a device. Only the main part is written.
- Locations as a hierarchy. The location is only inside the tag text.
- PLC racks and cards (see [docs/plc.md](plc.md)).
- Panel layouts.

## IDs are not stable

The `d1`, `d2`, ... IDs follow the sorted order of the tags, and `c1`, `c2`, ... the sorted pin
names. Adding a device or a pin can renumber the ones after it, so a receiving tool should match
devices by `ReferenceDesignation` (or the element name), not by ID.

## Warnings

- `symbol <id> has no vendored copy (run `wiringcraft lib lock`)`: the symbol is not in the
  project's library copy, so its pins are not written. Only the CLI prints this.
- `<n> connected pin(s) on symbols without a tag were left out`: wired pins on untagged symbols
  (such as a PE rail symbol). The app shows the same note after its export message.

The CLI prints both warnings on stderr and then a `wrote <file>: <n> devices, <n> connections`
line. The app reports `Exported <n> devices and <n> connections to <file>`.

## Reading it back

`wiringcraft import aml <file>` (the AutomationML import in the app's File menu) reads an AutomationML file
as overview pages. See [docs/plc.md](plc.md) for the PLC side of the export.
