# Troubleshooting

Common problems and what to do about them. Each one quotes the message WiringCraft prints or
shows, so you can search this page for the text you see. Command-line messages go to stderr and
start with `error:` or `warning:`.

## Where the app writes its log

The desktop app keeps a log, `wiringcraft.log`, in the `logs` folder of its settings folder:
`$XDG_CONFIG_HOME/wiringcraft/logs` when that variable is set, otherwise
`%APPDATA%\wiringcraft\logs` on Windows and `~/.config/wiringcraft/logs` elsewhere. It holds up
to five files of 1 MB (`wiringcraft.log`, then `.1` to `.4`). Home folders, API keys, tokens,
email addresses and device tags are masked before a line is written. Help, Report a problem
shows the log before anything is sent and has a "Show the log folder" button.

## The app does not start, or OpenGL is too old

The app draws with OpenGL by default. A remote desktop session, a virtual machine or a missing
graphics driver often offers only OpenGL 1.1, and the window cannot start. The app then starts
itself again with wgpu (Direct3D, Metal or Vulkan, with a software fallback) and prints:

```
wiringcraft-app: OpenGL could not start (<reason>); starting again with wgpu
```

If the second start fails as well, it prints
`wiringcraft-app: could not start again with wgpu: <reason>`.

To pick the backend yourself, set `WIRINGCRAFT_RENDERER` before starting the app:

- `WIRINGCRAFT_RENDERER=wgpu` starts on wgpu at once and skips the OpenGL try.
- Any other value (for example `glow`) starts on OpenGL and turns the automatic fallback off.

Start the app from a terminal to see these lines.

## A DWG drawing does not import

`wiringcraft import page-dwg` (and the app's import) first reads the file with WiringCraft's own
DWG reader, which reads AutoCAD 2004, 2007, 2010, 2013 and 2018 drawings. A file it refuses is
handed to a converter that turns it into DXF. With no converter installed you get both reasons:

```
error: WiringCraft's own DWG reader refused the file (DWG: drawing version AC1015 is not supported; save it as AutoCAD 2018, 2013, 2010, 2007 or 2004); no DWG converter found: install ODA File Converter or dwg2dxf, or set WIRINGCRAFT_DWG2DXF
```

What to do:

- Save the drawing again in AutoCAD 2018, 2013, 2010, 2007 or 2004 format, or
- install a converter: `dwg2dxf` (from LibreDWG) or the ODA File Converter
  (`ODAFileConverter`), on the `PATH`, or
- set `WIRINGCRAFT_DWG2DXF` to your own command, with `{in}` for the DWG file and `{out}` for
  the DXF to write, for example `WIRINGCRAFT_DWG2DXF="my-converter {in} {out}"`. When it is set,
  only that command is tried. If it does not write the DXF, the message ends with
  `WIRINGCRAFT_DWG2DXF did not produce a DXF`.

A file that is not a DWG drawing at all is refused with
`not a DWG drawing (it does not start with a version)`. `wiringcraft inspect` lists DWG files in
an archive as `DWG drawing (needs a converter)`.

## The assistant cannot connect

The assistant panel connects in one of two ways, picked under "Connect with" in its settings:
"My Claude login" (Claude Code on this computer) or "API key".

**My Claude login, connector not installed.** WiringCraft starts Claude Code through its
connector, `claude-agent-acp` (or the older `claude-code-acp`), found on the `PATH` or in the
usual npm, Volta, Bun and Homebrew folders; failing that it runs it through `npx`. When none of
these is found the panel says:

```
Claude Code's connector was not found. Install Claude Code and its connector in a terminal:

  npm install -g @agentclientprotocol/claude-agent-acp

then log in once with `claude` and /login. Or switch Settings to an API key.
```

Run that command, or type the full command line of the connector in the settings' "Command"
field.

**My Claude login, not logged in.** When Claude Code reports that the account is not logged in,
the panel says:

```
Claude Code is not logged in on this computer. Open a terminal, run `claude`, type /login and sign in with your Claude account, then send your message again.
```

**API key, no key.** The key comes from the `ANTHROPIC_API_KEY` environment variable, which wins
over a key pasted in the assistant settings. With neither:

```
No API key: set ANTHROPIC_API_KEY or add one in the assistant settings.
```

**API key, refused.** A key the API does not accept gives
`The API key was not accepted (<detail>).`, a key that may not use the chosen model gives
`This API key may not use this model or feature (<detail>).`, and a network problem gives
`Could not reach the API: <reason>`. `Too many requests right now; try again shortly (<detail>).`
means the API is rate limiting you; wait and send again.

## "is not in any library source" when placing a symbol

```
error: Symbol "iec.contactor.coil" is not in any library source
```

The project's library sources have no symbol (or part) with that id. Check the id with
`wiringcraft lib search <text>`, which lists each item with the source it comes from. A project
made with `wiringcraft new` has no library sources; add one with
`wiringcraft lib add <name> folder <path>` (or `git <url>`), for example the `library` folder
that comes with WiringCraft (its symbol ids start with `wc.`, such as `wc.coil`). A blank project made in the app gets
the starter library that ships next to the app, when there is one.

If `wiringcraft check` reports `symbol <id> is missing from the project's library copy`, the
project's `vendor/` copy is gone or unreadable; `wiringcraft lib lock` writes it again.

## A command says it is waiting for another command

```
waiting for another wiringcraft command on this project to finish
```

Command-line commands take turns on a project through the lock file `.wiringcraft.lock` in the
project folder. The second command waits until the first one ends, then runs. If it keeps
waiting, another `wiringcraft` process is still working on that project (a script, a git hook, a
command in another terminal): let it finish or stop it. The lock is released when the process
ends, even by a crash, so a left-over `.wiringcraft.lock` file does not block anything and need
not be deleted. It is left out of git.

When the lock file cannot be opened or locked (a read-only folder, say), the command runs anyway
and warns:

```
warning: running without the project lock (<path>: <reason>); another wiringcraft command on this project at the same time could undo this one's changes
```

## A project from a newer WiringCraft is refused

```
error: <folder>/project.toml is project format 2, newer than this WiringCraft reads (format 1): update WiringCraft to open it
```

`format_version` in `project.toml` is higher than this WiringCraft understands. Update
WiringCraft. Do not lower the number by hand: the project may hold data the older version would
drop.

## Git is not available

Projects are git repositories, and `wiringcraft new` and every `wiringcraft vcs` command run the
`git` program. Without git on the `PATH`:

```
error: cannot run git: No such file or directory (os error 2)
```

Install git and make sure `git --version` works in the same terminal. `wiringcraft new` has
already written the project files when it reports this, so running `new` again says
`<folder> already holds a project`; run `wiringcraft -C <folder> vcs init` once git is installed
instead. `wiringcraft new --no-git` makes a project without a repository. Commits work without a
git identity configured: WiringCraft then commits as `WiringCraft <wiringcraft@localhost>`.

## No project in the folder

```
error: no WiringCraft project in <folder> (no project.toml): use -C <folder> or create one with wiringcraft new <folder>
```

The command ran in a folder without `project.toml`. Pass the project folder with `-C <folder>`
(after a subcommand that takes a device tag, write `-C <folder>` with a space, since an attached
`-C<folder>` is read as a tag), or create a project with `wiringcraft new <folder>`.

## The file picker does not open (Linux)

```
The file picker could not open. On Linux it needs a desktop portal (xdg-desktop-portal) or zenity in a desktop session.
```

Install `xdg-desktop-portal` (with the backend for your desktop) or `zenity`, then try again.
