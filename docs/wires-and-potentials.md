# Wires, potentials and wire numbers

A wire is a line between two ends on one page. WiringCraft works out which pins are connected
(the nets, also called potentials) from the wires alone, across all pages. This page explains
what a wire holds and how the connections are computed.

## What a wire holds

Each wire is stored in its page file (`pages/<id>.page.json`) with these fields:

| Field | What it is | Command line |
|---|---|---|
| `id` | the wire's id on the page, as `wire` prints it (`w1`) | printed by `wire` |
| `from`, `to` | the two ends: a pin (`item:pin`, such as `s1:13`) or a free point `x,y` in mm | `<FROM>` `<TO>` |
| `via` | bend points between the ends | `--via x,y` |
| `number` | the wire number, free text | `--number` |
| `potential` | a potential (signal) name, such as `L1` or `24V` | `--potential` |
| `cable` | the cable it is a core of, and the core's name | `--cable`, `--core` |
| `section` | cross-section with its unit, such as `1.5 mm²` or `18 AWG` | `--section` |
| `colour` | colour code, such as `BU` or `GNYE` | `--colour` |
| `type` | wire type, such as `H07V-K` | `--type` |

Cross-section, colour and type are free text, stored as typed (spaces around them are dropped).
They do not change the connections. They show in the wire list (`report wires`) and feed the
WAGO configurator export. A pin end names the placed symbol by its id on the page (`s1`), not
by its device tag.

## How connections are computed

The code (`crates/wc-core/src/connectivity.rs`) puts every pin, free point and potential name
into one graph and joins them by these rules:

1. A wire joins its two ends.
2. Wire ends on the same pin are joined, since they are the same pin.
3. Free ends at the same point on the same page are joined. Points are compared to a hundredth
   of a millimetre.
4. Wires with the same potential name are joined, on the same page or on any other page.
   Spaces around a name are no part of it, and a blank name joins nothing.
5. Interruption points (symbols whose role is `interruption`, such as `wc.interruption`) with
   the same device tag are joined. Only wired ones take part.
6. A jumper (`jumper add -X1 3 4`) joins the same side of its terminals on any page: all wired
   pins `1` of the jumpered terminals are one net, all pins `2` another. The two sides of a
   terminal stay apart.

Each group that reaches at least one pin is a net. A group with no pin (for example a wire
between two free points) is no net, and its wires belong to no connection.

Some things do not connect:

- A free end that lies on the middle of another wire is not joined to it, even though a junction
  dot is drawn there. End both wires at the same point instead (see issue #1655).
- A free end placed where a pin is drawn is not joined to the pin. Draw the end on the pin
  (`item:pin`).
- Wires on two different pins of one symbol are not joined, except through a jumper.

## Listing the nets

`wiringcraft nets` prints one line per net: its potential name (or `net1`, `net2`, ... when it
has none) and its pins by device tag. Several names on one net are joined with `/`. The
reports (`report nets`, `report wires`) call a net without a name `N1`, `N2`, ... instead.

A net carrying two different potential names is a short: `check` reports it as an error
("potentials L1 and L2 are connected to each other").

## Wire numbers

A wire number names a connection. `wiringcraft number-wires` gives every wire of one net the
same number, so the number is the same on both sides of a junction and on every page. Rules
(`--rule`, default `page-column`):

| Rule | Number |
|---|---|
| `sequential` | 1, 2, 3 in drawing order: page by page, left to right, top to bottom |
| `page-column` | page number and frame column where the connection starts (`3.4`); more connections starting in that column get `3.4.2`, `3.4.3` |
| `potential` | the net's potential name (`24V`) when it has one and it is not taken yet, else sequential |

A connection starts on the first page it is drawn on, at its leftmost point (then topmost),
counting wire ends and bend points.

A connection that already has a number on any of its wires keeps it, and its other wires take
it. `--overwrite` renumbers those too. A new number never repeats one in use. Wires that touch
no pin get no number, and neither do wires on overview pages.

A number typed with `--number` is not checked when the wire is drawn. `check` warns when one
connection carries several numbers ("one connection has wire numbers 1 and 2") and when one
number is on wires of several connections ("wire number 5 is on 2 connections").

## Example

Two coils on pages 2 and 3, joined by the potential name `24V`, and a blue 1.5 mm² wire:

```sh
wiringcraft wire p2 s1:14 s2:A1 --section "1.5 mm²" --colour BU
wiringcraft wire p2 s1:13 60,40 --potential 24V
wiringcraft wire p3 s1:A1 60,100 --potential 24V
wiringcraft nets
```

```text
net1: -S1:14 -K1:A1
24V: -S1:13 -K2:A1
```

`-S1:13` on page 2 and `-K2:A1` on page 3 are one net through the name. After
`wiringcraft number-wires`, both `24V` wires carry the number `2.2`. `report wires` lists the
free ends as `junction 60.0,40.0`, and the section and colour in their own columns:

```text
Page  Wire   Potential  Cable  From    To                   Section  Colour  Type
2     2.2.2  N1                -S1:14  -K1:A1               1.5 mm²  BU
2     2.2    24V               -S1:13  junction 60.0,40.0
3     2.2    24V               -K2:A1  junction 60.0,100.0
```

The PDF prints a wire's number, potential name and cable core next to its first segment.

## Free ends and `check`

A free end that meets no other wire and whose wire has no potential name is "in the air":
`check` warns about it. A free end with a potential name is a potential arrow and is fine.
