# Versions and review from the command line

This page covers the commands for history and review: `vcs` (versions, branches, merges),
`comment` (review comments on pages), `revision` (revisions, clouds and the revision table),
`check` (the project check) and `rules` (the check rules). Every command works on the project in
the current folder; `-C <dir>` (or `--project <dir>`) points it at another one. Run
`wiringcraft <command> --help` for the full text of each option.

The examples use the pump starter project that `examples/demo.sh` builds, with pages `p0002`
(main circuit) and `p0003` (control circuit). Ids, dates and names in the output are examples.

## vcs: versions, branches and history

A project folder is a git repository. `wiringcraft new` makes one unless you pass `--no-git`.
For a project made with `--no-git`, `vcs init` makes it one later. It prints nothing:

```sh
$ wiringcraft vcs init
$ git log --oneline
78b1607 New project
```

| Command | What it does |
|---|---|
| `vcs init` | Make the project folder a git repository, with a first version "New project". |
| `vcs commit -m <text>` | Record every change in the project as a new version. Prints its short id, or `nothing to record`. |
| `vcs branch [name]` | List the branches (the current one marked `*`), or create one. Creating does not switch to it. |
| `vcs switch <name>` | Switch the project to another branch. |
| `vcs merge <name>` | Merge another branch into the current one. Conflicting files are listed. |
| `vcs merge --abort` | Undo a merge that stopped on conflicts, restoring the files. |
| `vcs log [--limit N] [--svg <file>]` | History as a subway map in the terminal, or written as SVG. At most 200 versions unless `--limit` says otherwise. |
| `vcs changes [from] [to]` | What changed, in schematic terms. With no arguments: the uncommitted changes since `HEAD`. `from` and `to` are commits, branches or tags. |
| `vcs page-log <page> [--limit N]` | One page's history, newest first: id, date, author, message and what changed on that page. The page is its id or number. At most 50 versions by default. |

Record a change and look at it:

```sh
$ wiringcraft place p0003 wc.lamp --at 210,150 --tag "=PUMP+CAB1-P2"
s10
$ wiringcraft vcs changes
devices.json changed
page 3: wc.lamp =PUMP+CAB1-P2 added
$ wiringcraft vcs commit -m "Second lamp"
f3c060a8ec
$ wiringcraft vcs page-log p0003 --limit 1
f3c060a8ec  2026-11-27  Ann  Second lamp
    page 3: wc.lamp =PUMP+CAB1-P2 added
```

Work on a branch and merge it back:

```sh
$ wiringcraft vcs branch rework
$ wiringcraft vcs switch rework
$ wiringcraft device "=PUMP+CAB1-P1" --description "Pump on"
$ wiringcraft vcs commit -m "Rename lamp"
5a13c0b910
$ wiringcraft vcs switch main
$ wiringcraft vcs merge rework
merged rework
$ wiringcraft vcs log
◉    5e9096b [main] Merge rework
│ ●  5a13c0b [rework] Rename lamp
●    a3390db Pump starter: main and control circuits
●    95647c4 New project
$ wiringcraft vcs log --svg history.svg
wrote history.svg
```

When both branches changed the same page, the merge stops and exits with an error:

```sh
$ wiringcraft vcs merge a
conflicts in:
  pages/p0003.page.json
error: resolve the files, then `wiringcraft vcs commit`, or undo the merge with `wiringcraft vcs merge --abort`
$ wiringcraft vcs merge --abort
merge aborted
```

Compare two versions instead of the uncommitted files:

```sh
$ wiringcraft vcs changes HEAD~2 HEAD
devices.json changed
page 3 renamed "Control circuit" -> "Control B"
page 3: wc.lamp =PUMP+CAB1-P2 added
```

## comment: review comments on pages

Comments are kept per page in the project's `comments` folder, so they are versioned with the
rest of the project. Each comment has an id such as `c1`.

| Command | What it does |
|---|---|
| `comment add <page> <text> [--at x,y] [--author name]` | Pin a comment on a page. `--at` is a point in mm; without it the comment is about the whole page. Prints the id. |
| `comment reply <id> <text> [--author name]` | Answer a comment. A reply opens a resolved comment again. |
| `comment resolve <id>` | Mark a comment resolved. |
| `comment reopen <id>` | Open a resolved comment again. |
| `comment remove <id>` | Delete a comment and its replies. |
| `comment list [page] [--all]` | List the comments of one page (by page id) or of all pages. Open ones only, unless `--all`. |

The author is `--author`, else the `WIRINGCRAFT_USER` environment variable, else git's
`user.name`, else the login name.

```sh
$ wiringcraft comment add p0003 "Is K1 rated for 24 V DC?" --at 60,150 --author Ann
c1
$ wiringcraft comment reply c1 "Yes, coil is 24 V DC" --author Bob
$ wiringcraft comment resolve c1
$ wiringcraft comment list --all
c1 page 3 at 60,150  Ann  2026-11-27  (resolved)
    Is K1 rated for 24 V DC?
    > Bob (2026-11-27): Yes, coil is 24 V DC
0 open, 1 resolved
$ wiringcraft comment reopen c1
$ wiringcraft comment list
c1 page 3 at 60,150  Ann  2026-11-27
    Is K1 rated for 24 V DC?
    > Bob (2026-11-27): Yes, coil is 24 V DC
1 open, 0 resolved
$ wiringcraft comment remove c1
$ wiringcraft comment list --all
0 open, 0 resolved
```

## revision: revisions, clouds and the revision table

Revisions are kept in `project.toml` as `[[revision]]` tables, in the order they were added.

| Command | What it does |
|---|---|
| `revision list` | List the revisions (letter, date, author, text), then the clouds on each page with their index, and where a revision table is shown. |
| `revision add <letter> [--text t] [--date d] [--by name]` | Add a revision after the others. The date is free text, printed as given. |
| `revision edit <letter> [--text t] [--date d] [--by name]` | Change a revision's description, date or author. |
| `revision remove <letter>` | Remove a revision. Refused while a cloud on any page carries it. |
| `revision cloud <page> <x1> <y1> <x2> <y2> [--rev letter]` | Draw a cloud around the rectangle between two corners, in mm. Without `--rev` it carries the last revision in the list. Prints the cloud's index on the page. |
| `revision uncloud <page> <index>` | Remove the cloud at that index (as `revision list` shows it, counted from 0). |
| `revision uncloud <page> --rev <letter>` | Remove every cloud of that revision from the page. |
| `revision table <page> <x> <y>` | Show the revision table on a page, top-left corner at (x, y) in mm. |
| `revision table <page> --off` | Take the table off the page. |

The coordinates are separate arguments, not `x,y` pairs.

```sh
$ wiringcraft revision add A --text "First issue" --date 2026-11-20 --by Ann
$ wiringcraft revision add B --text "Second lamp" --date 2026-11-26 --by Bob
$ wiringcraft revision edit B --date 2026-11-27
$ wiringcraft revision cloud p0003 195 135 225 165
0
$ wiringcraft revision cloud p0003 40 130 80 170 --rev A
1
$ wiringcraft revision table p0003 300 10
$ wiringcraft revision list
A	2026-11-20	Ann	First issue
B	2026-11-27	Bob	Second lamp
page p0003 cloud 0: revision B at 195.0,135.0 to 225.0,165.0
page p0003 cloud 1: revision A at 40.0,130.0 to 80.0,170.0
page p0003 table at 300.0,10.0
$ wiringcraft revision remove A
error: page 3 has a cloud of revision A; remove it first
$ wiringcraft revision uncloud p0003 --rev A
$ wiringcraft revision remove A
```

## check: find mistakes

`wiringcraft check` runs the built-in checks (open pins, wires ending in the air, devices drawn
twice, missing parts, shorted potentials and more; `wiringcraft check --help` lists them all),
then the house rules of each library source, then the project's own rules. It prints one finding
per line and a count. Notes (such as open pins and devices without a part) are only listed with
`--all`, but they are always counted.

It exits with an error when it finds at least one error. Warnings and notes do not fail it.

```sh
$ wiringcraft check
error   page 2: =PUMP+FIELD-M1 has no cable (check rule 1)
warning page 2: =PUMP+CAB1-K1 draws 3 main, 2 NO contacts, its parts have 3 main, 1 NO (1 NO too many)
1 error, 1 warning, 4 notes
error: the project check found 1 error
$ echo $?
1
```

### Guard a commit or a CI job with check

A git pre-commit hook that runs the check stops a commit while the project folder has errors. Git runs
the hook for `wiringcraft vcs commit` as well as for a plain `git commit`. The check reads the files
in the project folder, not what is staged. `wiringcraft vcs commit` stages every change first, so
the two match. For a plain `git commit`, stage every fix before you commit, or the hook can let
through staged files that still have errors. In the project folder:

```sh
cat > .git/hooks/pre-commit <<'EOF'
#!/bin/sh
exec wiringcraft check
EOF
chmod +x .git/hooks/pre-commit
```

Now a commit with an error in the project is refused, and the check's findings are printed:

```sh
$ wiringcraft vcs commit -m "Second lamp"
error: git commit -q --allow-empty -m Second lamp: error   page 2: =PUMP+FIELD-M1 has no cable (check rule 1)
...
error: the project check found 1 error
```

In a CI job, run the check as one step, with `wiringcraft` on the `PATH`. The step fails when the
check finds an error:

```sh
wiringcraft check -C path/to/project
```

## rules: the check rules

There are two kinds of rules:

- The project's own rules, `[[rule]]` tables in `project.toml`, numbered 1, 2, 3.
- House rules in a library source's `rules.toml` (same `[[rule]]` tables), named
  `source:number`, for example `house:1`. A project can turn one off; the list of rules turned off
  is `rules_off` in `project.toml`.

A rule has a `kind`: `cable` (the picked devices have a cable on one of their pins), `tag` (the
picked tags match `pattern`) or `max-wires` (at most `max` wire ends on one pin; it picks every
terminal unless told otherwise). It can also have `devices` (a tag pattern), `role` (`main`,
`auxiliary`, `standalone`, `terminal`, `interruption`), `severity` (`error`, `warning`, the
default, or `note`), `message` (`{tag}` in it is the device's tag) and `name`. In tag patterns
`*` is any run of characters, `?` one character and `#` a number; a pattern starting with `-`
also matches the product part of a tag with a function or location, so `-M*` matches `=PUMP+FIELD-M1`.

| Command | What it does |
|---|---|
| `rules list [--json]` | List the project's rules, then each library source's rules with `on` or `off`. `--json` prints `rules`, `library` and `rules_off`. |
| `rules set <file>` | Replace the project's rules with the ones in a `.toml` file (`[[rule]]` tables), a `.json` file (a list of rules, or `{"rules": [...]}`), or `-` for TOML on stdin. A rule the check could not run is refused and nothing changes. An empty file removes them all. |
| `rules off <source:number or source:name>` | Turn a library rule off for this project. |
| `rules on <source:number or source:name>` | Turn it back on: every `rules_off` entry that turns it off is removed. |

```sh
$ cat rules.toml
# Every motor has a cable on one of its pins.
[[rule]]
kind = "cable"
devices = "-M*"
severity = "error"

# At most two wires on one terminal pin.
[[rule]]
kind = "max-wires"
max = 2
$ wiringcraft rules set rules.toml
2 check rules
$ wiringcraft rules list
1	on	cable devices=-M* severity=error
2	on	max-wires max=2
house:1	on	tag name=terminal-tags role=terminal pattern=-X#.#
$ wiringcraft rules off house:terminal-tags
house:terminal-tags is off
$ wiringcraft rules on house:1
house:1 is on
$ printf '[[rule]]\nkind = "max-wires"\n' | wiringcraft rules set -
error: Rule 1: say how many wires a pin may have at most.
```

Here `house` is a library source added with `wiringcraft lib add house folder <dir>`, whose
folder holds a `rules.toml`.
