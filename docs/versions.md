# How versions work

A WiringCraft project folder is a git repository. Every version you record is a git commit,
every branch is a git branch, and the history you see in the app is git's history drawn as a
subway map. There is no separate database: if you know git, you already know where everything
is kept.

This page explains the ideas. For the commands and their options, see
[docs/cli/versions-and-review.md](cli/versions-and-review.md).

## The project folder is the repository

`wiringcraft new` makes the project folder a git repository on a branch called `main` and
records a first version, "New project". `wiringcraft new --no-git` skips that; you can turn
such a folder into a repository later with `wiringcraft vcs init`. Until then the `vcs`
commands refuse with "not under version control; run `wiringcraft vcs init` first".

A new repository also gets two small files:

- `.gitattributes` fixes the line endings of `*.json`, `*.toml` and `library.lock` to LF, so
  the same project gives the same bytes on Windows and Linux, and marks `*.pdf` as binary.
- `.gitignore` leaves out temporary files (`*.tmp`), the `export/` folder and
  `.wiringcraft.lock`, the file wiringcraft commands use to take turns on a project. The lock
  file is also listed in git's own local exclude list, so it stays out even of a repository
  whose `.gitignore` does not list it.

## What a version stores

A version is a snapshot of the whole project folder: every page, the devices, the project
settings, the library lock and the library items copied into the project. Recording a version
(Versions > Commit in the app, Ctrl+K, or `wiringcraft vcs commit -m "..."`) saves the project
and commits every change in the folder. When nothing changed, no version is made ("Nothing
changed since the last version." in the app, "nothing to record" in the CLI).

Each page is its own text file, `pages/<id>.page.json`, written the same way every time. That
is what makes git useful here: a change to one page touches one file, and a diff of a page
shows only what changed on it.

The author of a version is your git identity (`user.name` and `user.email`). When git has
none set, WiringCraft records the version as `WiringCraft <wiringcraft@localhost>` instead of
failing. In a share session every change is recorded as its own version with the participant
who made it as the author (see [docs/share-sessions.md](share-sessions.md)).

## Reading history in schematic terms

Git compares lines of JSON. WiringCraft reads the two versions of each page and tells you what
changed in schematic terms: pages added, removed, renamed or renumbered, a page's settings,
symbols added, removed, moved, retagged or replaced by another library symbol, wires added,
removed or changed, and drawn graphics. Any other file (devices, settings) is listed by name.

```console
$ wiringcraft vcs changes
page 2 renamed "Power" -> "Power supply"
```

`vcs changes` with no arguments compares the last version with the files as they are now;
give it two versions (commits, branches or tags) to compare those. In the app, Page history
shows, for the open page, who changed it, when, and what they changed;
`wiringcraft vcs page-log <page>` prints the same.

## Branches

A branch is a line of work that goes its own way: a variant for another customer, or a change
you want to try before it goes into `main`. Creating a branch (Versions > Branch, or
`wiringcraft vcs branch <name>`) starts it at the current version and switches to it at once.
Changes you have not recorded yet come along unchanged.

Switching to another branch (`wiringcraft vcs switch <name>`) replaces the project files with
that branch's latest version. It refuses while you have unrecorded changes, so nothing is lost:
record them first.

## The subway map

History shows every branch of the project at once as a subway map, newest version at the top.
Each branch is a coloured line and each version is a station on it. Where a branch starts, a
line splits off; where one is merged, it joins another line from the side.

```console
$ wiringcraft vcs log
◉    2fe4086 [main] Merge motor
│ ●  deebc62 [motor] Rename power page
│ ◆  dc93a03 Merge main
● │  056902c Rename control page
● │  e401339 Add power and control pages
●    aa46da8 New project
```

In the terminal `◉` is the version you are on, `◆` is a merge and `●` any other version.
Branch and tag names are shown in brackets at the version they point to. In the app the map
opens from Versions > History; `wiringcraft vcs log --svg map.svg` writes it as an SVG file.

## Merging

Merging brings the work of another branch into the one you are on
(`wiringcraft vcs merge <branch>`). A merge always makes a version of its own, "Merge
<branch>", so the map shows where the lines joined. Like switching, it refuses while you have
unrecorded changes, and while an earlier merge is still waiting to be finished.

Because each page is a separate file, two people who change different pages never conflict:
git takes both changes. A conflict happens only when both branches changed the same page file
in the same place, for example both gave page 3 a new title. The merge then stops and lists
the files:

```console
$ wiringcraft vcs merge motor
conflicts in:
  pages/control.page.json
error: resolve the files, then `wiringcraft vcs commit`, or undo the merge with `wiringcraft vcs merge --abort`
```

You then have two ways out:

- Resolve it. Open the listed files in a text editor or any git merge tool, keep the version
  you want of each conflicting part, and record a version with `wiringcraft vcs commit`. That
  version finishes the merge.
- Undo it with `wiringcraft vcs merge --abort`, which puts the files back as they were before
  the merge.

Until you do one of these, switching branches is refused.

## Working with plain git

Since the folder is an ordinary repository, plain git and the tools around it work too: push
the project to GitHub or any other server, review changes in pull requests, use your own git
client, or run `wiringcraft check` in a hook or CI job. WiringCraft does not keep anything
outside the folder that git would miss, and the commits it makes are ordinary commits that
other git tools read like any other.

A few habits keep a project easy to work on with git:

- Record versions often, with a message that says what changed and why. The message is what
  page history and the subway map show.
- Split work by page where you can. Different pages merge cleanly; the same page edited on two
  branches is where conflicts come from.
- Leave `.gitattributes` in place, so line endings stay the same on every computer.
