# Examples

## demo.sh

`demo.sh` builds a small pump starter project from the command line and exports it to PDF.
It shows most of the CLI in one go: a new project, the starter library, pages, symbols,
wires with numbers, potentials and cables, interruption points, terminals, device parts,
version control, the PDF and the reports.

Run it from the repository root:

```sh
sh examples/demo.sh                  # writes the project to target/demo
sh examples/demo.sh /tmp/pump        # or to a folder you pick
```

By default every command goes through `cargo run -p wc-cli`, so the first run builds the CLI.
To use a binary you already built, set `WIRINGCRAFT` to its absolute path:

```sh
WIRINGCRAFT=$PWD/target/debug/wiringcraft sh examples/demo.sh
```

The output folder is deleted first, so running it again starts over.

What it makes in the output folder:

- A project named "Pump station P1" (a git repository) with three pages: the cover page
  from `new`, page 2 "Main circuit" (breaker, contactor main contacts, three-phase motor)
  and page 3 "Control circuit" (stop and start buttons with self-hold, contactor coil and
  a lamp wired through terminals `-X1.1` and `-X1.2` and cable `-W1`).
- The starter library from `library/` added as a folder source and locked in `library.lock`,
  with the symbols and parts it uses copied into `vendor/`.
- One commit, "Pump starter: main and control circuits", on top of "New project".
- `pump-station.pdf`: the pages followed by every report page (`--report all`), dated 2026-11-16.

It also prints the ids of what it places and draws, then the parts list, terminal plan and
cable list as tables, and the commit log.

Running `wiringcraft check` in the finished project gives no errors. It warns that `-K1`
draws two NO auxiliary contacts while its example part has one.

## parts.csv

`parts.csv` is a semicolon-separated parts list with the columns `import parts-csv` reads:
manufacturer, order number, description, symbol, and extra columns (here coil voltage and
rated current) that become part properties. The data is made up.

It holds the six parts `demo.sh` assigns to its devices. The starter library already has
parts with the same ids (`library/parts/`, with a few more properties), so the demo does
not import this file. Use it to try the
import:

```sh
wiringcraft import parts-csv examples/parts.csv --lib /tmp/mylib
```

This writes one JSON file per part into `/tmp/mylib/parts/`, named by part id
(for example `example.ex-ct-9-24dc.json`).
