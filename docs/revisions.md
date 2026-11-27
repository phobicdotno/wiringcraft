# Drawing revisions

A drawing revision is an issue of the drawings that you hand to someone: revision A for the
first issue, B after the next round of changes, and so on. WiringCraft keeps three things for
this:

- the **revision list**: the project's issued revisions,
- **revision clouds**: scalloped outlines on a page around what a revision changed,
- the **revision table**: a table on a page that lists the revisions.

The commands are in [docs/cli/versions-and-review.md](cli/versions-and-review.md) and what the
exported PDF holds is in [docs/pdf-output.md](pdf-output.md). This page explains how the parts
fit together.

## The revision list

Each revision has a letter, a date, a description of what changed and an author. Only the
letter is required. The list is kept in `project.toml` as `[[revision]]` tables:

```toml
[[revision]]
letter = "A"
date = "2026-11-20"
text = "First issue"
author = "JD"
```

Some things to know:

- The letter is a short name, up to 8 characters: `A`, `B`, `01`, `P1`. Each letter is used
  once in a project.
- Letters must print in the PDF as themselves, so only characters from Latin-1 are taken.
  `revision add Ω` is refused.
- The date is free text and is printed exactly as you type it.
- The list is in the order the revisions were added, not sorted by letter. The **latest**
  revision is the one added last.
- A revision cannot be removed while a cloud on some page still carries its letter. Remove
  those clouds first.

In the app the list is edited under File > Project properties, in the Revisions section.

## Revision clouds

A cloud marks an area of a page that a revision changed. It is a rectangle drawn with
scallops (half circles) along its sides, with a small triangle on its top-right corner that
holds the revision letter. Clouds belong to a page and are saved in that page's file under
`pages/`.

- A new cloud gets the latest revision unless you name another one. You need at least one
  revision in the list before you can draw a cloud.
- A cloud needs some width and height; a flat rectangle is refused.
- In the app, the Revision cloud tool draws one from two clicked corners. Select a cloud to
  change its revision, drag it to move it, or press Delete to take it off.
- Clouds are not on any editor layer, so hiding or locking a layer does not affect them.

If a cloud's letter is not in the revision list (for example after editing a page file by
hand), `check` and `pdf` warn about it:

```
warning page 2: a cloud carries revision C, which the project does not list
```

## The revision table

The revision table lists every revision in the project, oldest first, with the columns Rev,
Date, Description and By. A page shows it only when you place it there, with its top-left
corner at a point you choose. You can place it on as many pages as you like; each copy shows
the same list. A long description is cut to fit its column and ends in `...`.

In the app, the Revision table tool places it with one click. Drag it to move it and press
Delete to take it off the page.

## In the PDF

The PDF draws clouds and the revision table from the same shapes as the editor, so the page
prints as you see it. They go in the PDF's "Drawings and notes" layer. See
[docs/pdf-output.md](pdf-output.md) for the rest of the page layout.

The **Revision** box in the title block is a separate title-block field. It is not filled in
from the revision list: set it in File > Project properties, or for one page with
`page fields <page> revision=B`.

## Example

```
wiringcraft revision add A --date 2026-11-20 --text "First issue" --by JD
wiringcraft revision add B --date 2026-11-27 --text "Motor M2 added" --by JD
wiringcraft revision cloud p1 100 80 160 120
wiringcraft revision table p1 250 230
wiringcraft revision list
```

The cloud gets revision B, the latest. `revision list` prints:

```
A	2026-11-20	JD	First issue
B	2026-11-27	JD	Motor M2 added
page p1 cloud 0: revision B at 100.0,80.0 to 160.0,120.0
page p1 table at 250.0,230.0
```

## Revisions and versions

Revisions and versions are different things.

- A **version** is a git commit, made with `vcs commit` or Record a version in the app. You
  can make as many as you like while you work. They record every change to the project
  folder and let you go back, branch and merge. See [docs/versions.md](versions.md) and
  [docs/cli/versions-and-review.md](cli/versions-and-review.md).
- A **revision** is part of the drawing itself. It is what the people who receive the
  drawings see: a letter in a cloud, a row in the revision table.

Making a version does not add a revision, and adding a revision does not make a version. A
usual way to work is to add the revision, cloud the changes, then record a version so the
issued state is kept. `vcs changes` reports cloud and table changes as
`page 2: revision clouds changed` and `revision table changed`.
