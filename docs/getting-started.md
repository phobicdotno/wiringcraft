# Getting started on the command line

This walk-through builds a small project with the `wiringcraft` command: a start button that
pulls in a relay, and a relay contact that lights a lamp. It takes about ten commands. The
output below is real, cut down where it is long.

Build the command first, from the repository root:

```sh
cargo build -p wc-cli
export PATH="$PWD/target/debug:$PATH"
```

Every command works on the current folder. Add `-C <dir>` to work on another project folder.
Lines starting with "In the app" name the desktop app command that does the same step.

## 1. Create a project

```console
$ wiringcraft new lamp-test --name "Lamp test"
created lamp-test
$ cd lamp-test
$ wiringcraft page list
p0001      1      Cover (0 symbols, 0 wires), cover
```

The folder is also a git repository with one version, "New project". It starts with a cover page.

In the app: New project (Ctrl+Shift+N).

## 2. Add a library source

The repository's own `library/` folder holds a starter set of symbols (`wc.*`) and example
parts (`example.*`). A project made with `wiringcraft new` has no library source, so until you
add one, `place` stops with `Symbol "wc.lamp" is not in any library source`. (The app adds the
starter library to a blank new project by itself.) Add it as a folder source, giving its full path:

```console
$ wiringcraft lib add starter folder /path/to/wiringcraft/library
$ wiringcraft lib search lamp
symbol  wc.lamp                          starter      Indicator lamp
part    example.ex-lamp-24-gn            starter      Example EX-LAMP-24-GN Indicator lamp green 24 V (example data)
...
```

`lib search` finds symbols and parts by id, name, category, letter code or description.

## 3. Add a page

```console
$ wiringcraft page add p0002 --number 2 --title "Control circuit"
```

`p0002` is the page's id, which the next commands use. The sheet is A3 landscape unless you
pass `--size`.

In the app: New page (Ctrl+N).

## 4. Place symbols

Positions are in millimetres from the top-left corner of the sheet. Each command prints the new
symbol's id on the page (`s1`, `s2`, ...). A contact and a coil with the same tag belong to one
device.

```console
$ wiringcraft place p0002 wc.button.no --at 60,80 --tag=-S1
s1
$ wiringcraft place p0002 wc.coil --at 60,120 --tag=-K1
s2
$ wiringcraft place p0002 wc.contact.no --at 120,80 --tag=-K1
s3
$ wiringcraft place p0002 wc.lamp --at 120,120 --tag=-P1
s4
```

Write the tag as `--tag=-S1` or `--tag -S1`; both work, although the tag starts with a dash.

In the app: Symbol (S) places the symbol chosen in the library picker on the right.

## 5. Wire them

A wire end is either `symbol:pin` or a point `x,y`. A wire from a point is a supply feed; give it
a potential name.

```console
$ wiringcraft wire p0002 60,50 s1:13 --potential 24V
w1
$ wiringcraft wire p0002 s1:14 s2:A1 --number 1
w2
$ wiringcraft wire p0002 s2:A2 60,160 --potential 0V
w3
$ wiringcraft wire p0002 120,50 s3:13 --potential 24V
w4
$ wiringcraft wire p0002 s3:14 s4:X1 --number 2
w5
$ wiringcraft wire p0002 s4:X2 120,160 --potential 0V
w6
$ wiringcraft nets
net1: -S1:14 -K1:A1
24V: -S1:13 -K1:13
net3: -K1:14 -P1:X1
0V: -K1:A2 -P1:X2
```

`nets` lists the connected pins across all pages. Add `--via x,y` for bend points.

In the app: Wire (W), then click a pin or point, click any bends, and click a pin to finish.
A single click on a free point adds a bend; double-click a free point to end the wire there,
as for the wires to 0V and 24V above. Esc cancels.

## 6. Run the check

```console
$ wiringcraft check --all
note    -K1 has no part
note    -P1 has no part
note    -S1 has no part
0 errors, 0 warnings, 3 notes
```

Without `--all`, `check` prints only errors and warnings, and the count line. It exits with an
error when it finds an error, so it can guard a commit or a CI job.

In the app: Check (Ctrl+E).

## 7. Set device parts

Give each device a part from the library and a short description:

```console
$ wiringcraft device -S1 --part example.ex-pb-grn-no --description "Start"
$ wiringcraft device -K1 --part example.ex-ct-9-24dc --description "Lamp relay"
$ wiringcraft device -P1 --part example.ex-lamp-24-gn --description "Running"
$ wiringcraft device -P1
part         example.ex-lamp-24-gn
description  Running
$ wiringcraft check --all
0 errors, 0 warnings, 0 notes
```

The symbols and parts the project uses are copied into its `vendor/` folder and pinned in
`library.lock`.

In the app: select the symbol and pick the part under Part in the properties on the right.

## 8. Print a report

```console
$ wiringcraft report parts
Parts list
Pos  Qty  Manufacturer  Order number   Description                                                        Devices
---  ---  ------------  -------------  -----------------------------------------------------------------  -------
1    1    Example       EX-CT-9-24DC   Contactor 3-pole 9 A, 1 NO auxiliary, 24 V DC coil (example data)  -K1
2    1    Example       EX-LAMP-24-GN  Indicator lamp green 24 V (example data)                           -P1
3    1    Example       EX-PB-GRN-NO   Push button green, 1 NO (example data)                             -S1
```

Other reports are `contents`, `devices`, `wires`, `nets`, `terminals`, `cables`, `plc`,
`plc-cards`, `locations`, `networks` and `all`. `--format csv`, `md` or `xlsx` (with
`--out <file>`) change the output.

In the app: Reports (Ctrl+R).

## 9. Export the PDF

The project's `.gitignore` leaves out the `export/` folder, so write the PDF there to keep it out
of the next version. The folder must exist.

```console
$ mkdir -p export
$ wiringcraft pdf export/lamp-test.pdf --report parts
wrote export/lamp-test.pdf (3 pages)
```

The three pages are the cover, the control circuit and the parts list. `--report all` appends
every report.

In the app: PDF (Ctrl+P).

## 10. Commit a version

```console
$ wiringcraft vcs changes
devices.json changed
library.lock changed
project.toml changed
page 2 added (Control circuit)
vendor/parts/example.ex-ct-9-24dc.json added
...
vendor/symbols/wc.lamp.json added
$ wiringcraft vcs commit -m "Control circuit with start button, relay and lamp"
673a2bbd5f
$ wiringcraft vcs log
◉  673a2bb [main] Control circuit with start button, relay and lamp
●  530eb20 New project
```

`vcs commit` records every change in the project and prints the new version's short id. Your
ids will differ.

In the app: Commit (Ctrl+K).

## Next steps

- `wiringcraft --help` lists every command, and `wiringcraft <command> --help` its options.
- `examples/demo.sh` builds a larger motor starter over two pages, with terminals and a cable.
- To open the project in the desktop app: `cargo run -p wc-app -- path/to/lamp-test`.
