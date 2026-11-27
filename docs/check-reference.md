# Project check findings

Every finding the project check can report, with what causes it and how to fix it. The same
check runs in three places:

- `wiringcraft check` prints errors and warnings, and notes too with `--all`. Each line is the
  severity, the page number when the finding is on a page, and the message. It exits with an
  error when it finds an error, so it can guard a commit or a CI job. `wiringcraft check --help`
  lists every check it runs.
- **Check** (Ctrl+E) in the app lists every finding, notes included. Click a finding that is
  on a page to go to it.
- **Check while drawing** in the app runs the check again shortly after each edit, rings the
  items with errors and warnings on the canvas and counts them in the status bar. Notes are
  not marked.

Severities:

- **error**: the drawing or its reports cannot be trusted until it is fixed.
- **warning**: probably a mistake.
- **note**: worth knowing, often fine.

Findings come sorted: errors first, then by page, then by message. In the messages below,
`<tag>` is a device tag such as `-K1`, `<item>` a placed symbol's id on its page (such as
`s3`), and other `<...>` parts are filled in from the project.

Overview pages (line diagrams, `page add --overview`) show devices and connections again on
purpose, so their symbols and wires do not count for "drawn twice", contact counts, wire
numbers, cable cores or the check rules.

## Drawing

| Severity | Message | Cause | Fix |
|---|---|---|---|
| error | `symbol <symbol> is missing from the project's library copy` | A placed symbol's library id has no copy in the project. | Run `wiringcraft lib lock` to vendor it, add the library source that has it, or replace the symbol. |
| error | `wire ends on <item>:<pin>, which does not exist` | A wire end names a pin the symbol does not have, or a symbol that is not on the page. | Reconnect the wire end to a real pin, or delete the wire. |
| error | `<tag> is drawn <n> times (page <list>)` (or `pages <list>`) | One tag is on two coils (main symbols), or on two copies of the same standalone symbol or terminal. Reported once per copy. Contacts may repeat. | Give one copy another tag, or delete the extra copy. |
| warning | `<tag> draws <drawn> contacts, its parts have <has> (<over> too many)` | The contacts drawn for a device are more than its main part and accessories list. Only checked when a part lists its contacts. | Remove the extra contacts, or add an auxiliary contact block as an accessory. |
| warning | `contact <tag> has no coil` | A contact (auxiliary symbol) has a tag that no main symbol carries. | Draw the coil with that tag, or fix the contact's tag. |
| warning | `interruption point <tag> has no partner` | Only one interruption point carries this tag on schematic pages. A copy on an overview page does not count as a partner for one on a schematic page. | Place the partner interruption point with the same tag, or delete this one. |
| warning | `interruption point has no tag, so it joins nothing` | An interruption point without a tag. | Give it the tag of the point it continues to. |
| note | `<tag>: pin <pins> not connected` | No wire reaches these pins. A symbol without a tag is named by its symbol name. | Wire the pins if they should be used; often fine (spare contacts, unused motor pins). |
| warning | `wire <number> ends in the air at <x>, <y>` (no number when the wire has none) | A free wire end that touches no other wire and carries no potential name. | Connect the end to a pin or another wire, or give the wire a potential. |
| warning | `wire <number> starts and ends on <tag>:<pin>` | Both ends of a wire are on the same pin. It connects nothing but shows in the wire list and hides the open-pin note. When several wires on the page share the id, ` (wire <k> of <n> with id <id> on this page)` is added. | Delete the wire, or move one end. |

## Connections, wire numbers and cables

| Severity | Message | Cause | Fix |
|---|---|---|---|
| error | `potentials <A> and <B> are connected to each other` | One connected net carries two potential names (for example L1 and L2): a short. | Find the wire that joins them and delete or reconnect it, or correct a potential name. |
| warning | `one connection has wire numbers <list>` | Wires of one connection carry different numbers. Wire numbering never does this; typed or imported numbers can. Named on the connection's first wire. | Give every wire of the connection the same number, or run `wiringcraft number-wires`. |
| warning | `wire number <number> is on <n> connections` | One wire number is used on wires of separate connections. Named on the second connection, as the first may be the right one. | Renumber one of them by hand. Plain `wiringcraft number-wires` keeps numbers already set, so it leaves this as it is; `wiringcraft number-wires --overwrite` fixes it but renumbers every connection. |
| warning | `cable core <cable>:<core> is on <n> connections` | Wires on separate connections name the same core of one cable. A core is one conductor. | Give the wires different cores, or fix the connection. |
| warning | `jumper of <strip>: terminal <t> is drawn nowhere` | A jumper names a terminal number that is not drawn, not a spare terminal of the strip and not on an overview page. | Draw the terminal, mark it as a spare in the terminal strip editor, or take it out of the jumper (`wiringcraft jumper remove`). |

## Revisions

| Severity | Message | Cause | Fix |
|---|---|---|---|
| warning | `a cloud carries revision <rev>, which the project does not list` | A revision cloud names a revision that is not in the project's revisions. | Add the revision (`wiringcraft revision add`), or remove the cloud (`wiringcraft revision uncloud`). |
| warning | `revision <letter> has characters the PDF cannot print apart from others; give it plain Latin-1 letters` | A revision letter uses characters outside Latin-1. They print as `?` in the PDF and read alike. | Rename the revision with plain letters or digits. |

## Panel layouts

All start with `panel <name>: `.

| Severity | Message | Cause | Fix |
|---|---|---|---|
| warning | `panel <name>: rail at <x>,<y> sticks out of the plate` | A DIN rail is partly outside the mounting plate. | Move or remove the rail, or make the plate larger. |
| warning | `panel <name>: duct <x1>,<y1> to <x2>,<y2> sticks out of the plate` | A wire duct is partly outside the plate. | Move or remove the duct, or make the plate larger. |
| warning | `panel <name>: <tag> sticks out of the plate` | A device footprint is partly outside the plate. | Move the device onto the plate (`wiringcraft panel device`). |
| warning | `panel <name>: <tag> and <other> overlap` | Two device footprints cover each other. Outlines that only touch do not overlap. | Move one of them. |
| warning | `panel <name>: <tag> is on the plate but no longer in the project` | The tag is neither drawn on a page nor listed in the devices file. | Take it off the plate (`wiringcraft panel device <layout> <tag> --remove`), or fix its tag. |

## Parts

| Severity | Message | Cause | Fix |
|---|---|---|---|
| warning | `<tag>: part <part> is missing from the project's library copy` | A device's main part or accessory has no copy in the project. | Run `wiringcraft lib lock`, or set another part with `wiringcraft device <tag> --part <id>`. |
| warning | `<tag>: part <part> in the project's library copy cannot be read: <why>` | The part's copy in the project is there but broken. | Fix or delete the file and run `wiringcraft lib lock` to write it again. |
| warning | `jumper of <strip>: part <part> is missing from the project's library copy` (or `... cannot be read: <why>`) | The same for a jumper's part. | Run `wiringcraft lib lock`, or set another part with `wiringcraft jumper part`. |
| note | `<tag> has no part` | A device (cables included) has no main part, so the parts list shows it without a maker and its contacts cannot be checked. | Set a part with `wiringcraft device <tag> --part <id>`; fine while the design is early. |

## Check rules

House rules come from a `rules.toml` in a library source's folder and run before the
project's own `[[rule]]` tables in `project.toml`. A rule is named by its number from 1:
`check rule <n>` for a project rule, `check rule <n> of library <source>` for a house rule,
with `"<name>"` after the number when the rule has a name. Below, `<label>` stands for that.
Manage the rules with `wiringcraft rules` or the Rules dialog in the app.

### Findings of rules that run

The severity is the rule's `severity` (`error`, `warning` or `note`, warning when left out).
A rule with a `message` shows that text instead, with `{tag}` replaced by the device's tag;
for a house rule ` (<label>)` is still added.

| Kind | Message | Cause | Fix |
|---|---|---|---|
| `cable` | `<tag> has no cable (<label>)` | No wire with a cable ends on any of the device's symbols that the rule picks. With `role` set, only symbols of that role count: a cable on another symbol of the same tag does not. | Set a cable on a wire of a picked symbol (`wiringcraft wire ... --cable`), or narrow the rule's `devices` or `role`. |
| `tag` | `<tag> does not match <pattern> (<label>)` | A picked device's tag does not match the rule's pattern (`*` any run, `?` one character, `#` a number). | Rename the device, or change the pattern. |
| `max-wires` | `<tag>:<pin> has <n> wires, at most <max> allowed (<label>)` | More wire ends on one pin than the rule allows. Without `role` or `devices` the rule looks at terminals. | Spread the wires over more terminals, or raise `max`. |

### Rules that cannot run

These are errors, so a broken rule is never skipped quietly.

| Severity | Message | Cause | Fix |
|---|---|---|---|
| error | `<label> cannot run: it has a key "<key>" it does not know (name, kind, devices, role, pattern, max, severity and message are known)` | A misspelt or unknown key, such as `severty`. | Fix the key's spelling. |
| error | `<label> cannot run: it has no kind (cable, tag, max-wires)` | The rule has no `kind`. | Add `kind`. |
| error | `<label> cannot run: kind "<kind>" is not one of cable, tag, max-wires` | Unknown kind. | Use one of the three kinds. |
| error | `<label> cannot run: role "<role>" is not one of main, auxiliary, standalone, terminal, interruption` | Unknown role. | Use one of the listed roles, or leave `role` out. |
| error | `<label> cannot run: severity "<severity>" is not error, warning or note` | Unknown severity. | Use error, warning or note, or leave it out. |
| error | `<label> cannot run: a tag rule needs a pattern` | A `tag` rule without `pattern`. | Add `pattern`. |
| error | `<label> cannot run: a max-wires rule needs max` | A `max-wires` rule without `max`. | Add `max`. |
| error | `the check rules of library <source> cannot be read: <error>` | The source's `rules.toml` is not valid TOML or has the wrong shape. None of its rules run. | Fix the file. |
| warning | `rules_off names "<entry>", which is no library check rule here (write source:number or source:name)` | An entry in the project's `rules_off` turns off no house rule. | Fix the entry (`house:2` or `house:motor-cable`), or remove it with `wiringcraft rules on`. |
