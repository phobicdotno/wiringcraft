# Translations

A project is drawn in one language, its source language. Its texts can be translated into
other languages through a dictionary, and the PDF and the SVG or DXF drawings can then be
printed in one of those languages, or in two at once. The drawn pages themselves never
change: the translation is applied only when exporting.

## Which texts are translated

These texts are translated:

- page titles
- free texts on pages
- device descriptions
- location descriptions

Device tags, wire numbers, part numbers and title-block fields stay as they are.

A text is looked up as it is written, without its leading and trailing spaces. A text with no
word of two or more letters (such as `24 V`, `-K1` or `3~`) needs no translation and is never
listed as missing.

## Where translations are stored

The dictionary is `translations.toml` in the project folder, next to `project.toml`. A project
without the file simply has no translations. Each `[[text]]` table holds one text, keyed by
language code:

```toml
source = "en"

[[text]]
en = "Pumps"
nb = "Pumper"

[[text]]
en = "Cabinet"
nb = "Skap"
```

- `source` is the language the project is drawn in. When it is left out, it is `en`.
- `languages = ["de"]` names languages that have no translated text yet, so a language just
  added is kept. The app writes it; you do not need it when you edit the file by hand.
- Language codes are compared in lowercase, so `NB` and `nb` are the same language.
- An empty translation (`nb = ""`) counts as no translation.

The file is plain text and goes into the project's git history like the other project files.
While a change is being saved, the app and the command line hold
`translations.toml.lock.tmp` next to it, so two saves at the same moment take turns. Its name
ends in `.tmp`, so git leaves it out.

## The Translations form in the app

Open it with File, Translations.

- The bar at the top shows the source language ("Drawn in: en"). Type a code such as `nb` in
  "Add language" and press Add (or Enter) to add a column for it.
- Below the bar, each language shows how many texts it is missing.
- The table has one row per project text, the source text first and then one cell per
  language. Hover over a source text to see where it is used, such as `page 3 title` or
  `-K1 description`. Empty cells are marked "missing".
- "Only missing" hides the rows that already have every translation.
- OK saves the changed cells to `translations.toml`. Translations changed in the file by
  another program while the form was open are kept, as long as you did not change the same
  cell.

Undo and Redo take back and bring back translation edits made in the app or by its assistant.
In a shared session only the host can change the translations.

When a project has translations, File, Export PDF first asks for the language ("Language")
and an optional second one ("Shown with"), then where to save. The drawings export in the app
does not ask; use `wiringcraft drawing --lang` for translated drawings.

## Command line

`wiringcraft translations --lang <code>` lists the texts that have no translation into that
language, one per line with where each is used. It exits with an error when any text is
missing, so it can guard a release:

```console
$ wiringcraft translations --lang nb
Cover	page 1 title
error: 1 text(s) have no translation into nb
```

When nothing is missing it prints `every text has a translation into nb`.

Set or remove one text's translation. The text is given as drawn, in the source language:

```console
$ wiringcraft translations set "Pumps" "Pumper" --lang nb
set the translation in translations.toml
$ wiringcraft translations remove "Cabinet" --lang nb
removed the translation from translations.toml
```

- `set` makes `translations.toml` if the project has none, and keeps the other
  translations in the file as they are.
- `set` with an empty translation is refused; use `remove` instead.
- `remove` on a text that has no translation into that language is an error.
- The source language cannot be set (`en is the source language: its texts are the project's
  own`).

## Picking the language on export

`wiringcraft pdf` and `wiringcraft drawing` take `--lang`:

- `--lang nb` prints every translated text in `nb`.
- `--lang nb,en` (or `nb+en`) prints each text in `nb` with `en` after it, joined by ` / `:
  `Pumper / Pumps`. When both are the same, the text is printed once.
- More than two languages are refused.
- Without `--lang` the project is printed as drawn.

A text with no translation is printed as drawn, and the export warns about it but still
writes the file:

```console
$ wiringcraft pdf out.pdf --lang nb,en
warning: no translation for "Cover" (page 1 title)
wrote out.pdf (1 pages)
```

Run `wiringcraft translations --lang nb` first if a missing translation should stop the
export.

`wiringcraft export wago-marking --lang` is a different option: it sets the language of the
marker list's header row and type column (en, de, nb), not of the project's texts.
