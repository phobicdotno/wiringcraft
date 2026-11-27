# Fieldbus networks and topology pages

A project can list its fieldbus networks (PROFINET, EtherCAT or any other bus): which devices
are on each one, their addresses, and the cables between their ports. WiringCraft prints them
as the Networks report and draws one topology page per network in the PDF.

Networks are written by hand in `project.toml`; no command and no window in the app edits
them. Renaming a device also renames it in the nodes, link ends (the port is kept) and link
cables of every network.

## Nodes and links

Each network is a `[[network]]` table with its `[[network.node]]` and `[[network.link]]`
tables:

```toml
[[network]]
name = "PN1"
kind = "profinet"
description = "Line 1 field devices"

[[network.node]]
device = "-A1"
address = "192.168.0.1"
station = "plc-line1"

[[network.node]]
device = "-K10"
address = "192.168.0.10"

[[network.link]]
from = "-A1:X1P1"
to = "-K10:X1P1"
cable = "-W20"
```

| Key | What it holds |
|---|---|
| `name` | the network's name; the only key a network must have |
| `kind` | the bus type, free text. `profinet` or `pn` is shown as PROFINET and `ethercat` or `ecat` as EtherCAT, in any letter case; anything else is shown as written |
| `description` | free text, printed in the headline of the topology page |
| node `device` | the device's tag, such as `-K10` |
| node `address` | its bus address: an IP address, a station number |
| node `station` | the PROFINET station (device) name, or another name the bus knows it by |
| link `from`, `to` | the two ends, each `<device tag>:<port>` or only the tag. The port is what follows the last colon |
| link `cable` | the cable's device tag, such as `-W20` |

Only `name`, a node's `device` and a link's `from` and `to` are needed. Keys WiringCraft does
not know are kept when the project is saved.

## The Networks report

`wiringcraft report networks` prints the list, `--report networks` adds it to the PDF, and
the app shows it with the other reports. Its columns are Network, Type, Device, Description,
Address, Station, Port, Connected to, Cable and Pages.

- Networks and their devices come in the order they are written.
- A device has one row for each link end on it, with its own port, the other end and the
  cable. A device without links has one row.
- Description is the device's description in the project.
- Pages lists the schematic pages the device is drawn on. Overview pages are not counted.
  In the PDF a row opens the lowest numbered of those pages.
- A node whose `device` is not a device tag is not listed, and a link with an end that cannot
  be read is left out. Both are warned about.

## Warnings

`wiringcraft pdf` and `wiringcraft report` (with any report) check the networks and print one
`warning:` line for each problem:

| Warning | When |
|---|---|
| `a network has no name` | `name` is empty or only spaces |
| `network PN1: a second network has this name` | two networks share a name (spaces around it do not count) |
| `network PN1: node "...": ...` | a node's `device` is not a device tag |
| `network PN1: -K10 is listed twice` | two nodes name one device |
| `network PN1: -K10 and -K11 have the same address 192.168.0.10` | two nodes share an address |
| `network PN1: -A1 and -K10 have the same station plc` | two nodes share a station name; `PLC` and `plc` are the same name |
| `network PN1: -K2 is not a device of the project` | a node's device is neither drawn nor listed in the project |
| `network PN1: link 3 is on -K99, which is not a node` | a link end names a device that is not a node of the network |
| `network PN1: link 2: ...` | a link end is not a device tag with an optional port |
| `network PN1: link 4 joins -K11:P2 to itself` | both ends are the same port |
| `network PN1: port -A1:X1P1 has 2 links` | a named port is used by more than one link |

Links are numbered from 1 in the order they are written.

Drawing the topology pages adds these, in `wiringcraft pdf` and in the message the app shows
after Export, PDF:

| Warning | When |
|---|---|
| `network PN1: node 2 is not a device tag: ...` | the node is drawn as written, in a box of its own |
| `network PN1: link 2: its from end is not a device or port: ...` | the same for a link's `from` or `to` end |
| `network PN1: link 2 is not drawn: an end names no device` | an end is empty |
| `network PN1: its topology needs 2400 x 900 mm, more than the largest sheet; some devices are drawn off the sheet` | the drawing is larger than 2000 mm a side |

The app's Export, PDF shows only this second group.

## The topology page

Each network gets one sheet, numbered `N1`, `N2`, ... in the order the networks are written,
with the title `Network <name>`. Its headline is the name, the bus type in brackets and the
description: `Network PN1 (PROFINET): Line 1 field devices`.

Every device is a box with up to four lines: its tag, its description in the project, its
address and its station. Every node gets a box, two nodes of one device share it, and a device that only a link
names gets one too. A node with an empty `device` is drawn as `(node 2)`, with its number among the nodes.
A box opens a page in the file where the device is drawn.

The boxes are placed from the links:

- The first node (without nodes, the first link end) goes at the top left. Every device linked to it goes one column to the
  right, the devices linked to those one column further, and so on. A line of devices runs
  across the sheet and a switch fans out into a tree.
- A part of the network that is not linked to what is already placed starts in the first
  column, below it.
- A link between two devices that are both placed already, such as the one that closes a
  ring, is drawn once: down beside the boxes and along a lane of its own under them.
- A link between two ports of one device is a small loop over its box.
- Links out of one device share one vertical run. Runs of different devices that would meet
  get tracks of their own, so two cables never look joined.

Each link carries its two port names, one at each end, and its cable tag. A short lane and a
loop over a box have them on one line, `X1P1 - X1P2 -W20`.

Tags, port names and cable tags are never cut. One that is too long for its place is drawn in
smaller type, down to 1.6 mm. If it is still too long, a key such as `[1]` is drawn in its
place and the whole text is written in a numbered legend under the drawing. An address is
also drawn in smaller type first, and is cut with `...` only if it does not fit at 1.6 mm. A
description or station name too long for the box is cut with `...`.

A network without nodes and links gets a sheet that says `No devices on this network.`

The sheet size and the frame are described in [What the PDF contains](pdf-output.md).

## When it is exported

| Export | Networks report | Topology pages |
|---|---|---|
| `wiringcraft pdf out.pdf --report networks` | always, also when it is empty | one per network |
| `wiringcraft pdf out.pdf --report all` | when the project has networks | one per network |
| `wiringcraft pdf out.pdf` with other reports or none | no | no |
| Export, PDF in the app | when it has rows | one per network |

The topology pages are the last sheets in the file, after the report tables. `--page` does
not leave them out; a box then opens a page only if that page is in the file.
