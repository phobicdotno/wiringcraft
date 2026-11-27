# Assistant

The assistant is a chat panel where you tell Claude what to draw or change in plain words. It
works in the open project while you watch, through WiringCraft's own tools (listed at the end).
Everything it changes in the project in one turn is one undo step (Ctrl+Z), except a
translation the host sets or removes in a shared session: the session's history does not hold
`translations.toml`, so Undo does not take that back. What happens outside the open project is
not undone that way either: a commit or branch made with `versions`, exported files, and a todo
filed with `todo` stay.

Open it with View → Assistant, Ctrl+J, or the speech-bubble button at the end of the toolbar.
**New** starts a new conversation, **Stop** stops a running turn, and **Settings** shows the
connection settings. **Paste screenshot** (or Ctrl+V in the box on Windows and Linux, or dropping
a PNG or JPEG file on the window) sends up to 5 pictures with your next message.

## Setup

Under Settings, **Connect with** picks one of two ways to reach Claude.

### My Claude login (default)

WiringCraft starts Claude Code through its Agent Client Protocol connector
(`claude-agent-acp`) and hands it the WiringCraft tools. The chat runs on your own Claude plan
(Pro, Max, Team) and no API key is needed. WiringCraft never sees a password or key.

One-time setup in a terminal:

```sh
npm install -g @anthropic-ai/claude-code               # Claude Code itself (the `claude` command)
npm install -g @agentclientprotocol/claude-agent-acp   # the Claude Code connector
claude                                                 # then type /login and sign in
```

The connector package does not install the `claude` command, so skip the first line only when
Claude Code is already installed.

WiringCraft looks for `claude-agent-acp` (or the older `claude-code-acp`) on your `PATH` and in
the usual install folders (`~/.local/bin`, `~/.npm-global/bin`, `~/.npm-packages/bin`,
`~/.volta/bin`, `~/.bun/bin`, `~/.claude/local`, `~/bin`, `%APPDATA%\npm` on Windows, and
`/opt/homebrew/bin`, `/usr/local/bin`, `/home/linuxbrew/.linuxbrew/bin`, `/usr/bin` elsewhere).
If none is found but `npx` is, it runs `npx -y @agentclientprotocol/claude-agent-acp`. The
settings show which command it found. If it is not logged in yet, the settings and the chat say
to run `claude` and `/login`.

### API key

Choose **API key** under Settings, then paste an Anthropic API key there or set
`ANTHROPIC_API_KEY`. When the variable is set it wins over a pasted key. Setting the variable
alone does not switch to the API key: the choice under Settings decides. A pasted key is kept in
your WiringCraft preferences on this computer, never in a project. Use is billed per request.

## Settings

The settings are saved in your WiringCraft preferences (`prefs.toml` in the settings folder, see
below), never in a project.

| Setting | Applies to | What it does |
| --- | --- | --- |
| Connect with | both | My Claude login or API key. Default: My Claude login. |
| Command | Claude login | The command line that starts the connector, when not the one found. Empty: found automatically. Quotes keep spaces in a word; no shell is involved. |
| Model | Claude login | One of the models Claude Code offers. The list shows once Claude Code is running. Empty keeps Claude Code's default. |
| Model | API key | Claude Opus 5.5 (default), Claude Sonnet 5.5 (faster), Claude Fable 5.2 (most capable) or Claude Haiku 5.5 (fastest). |
| Effort | API key | `low`, `medium`, `high` (default), `xhigh` or `max`. Higher is more thorough and slower. |
| API key | API key | The pasted key, hidden as you type. Not shown when `ANTHROPIC_API_KEY` is set. |

With an API key the assistant pauses after 60 requests in one turn and asks you to say "go on".
This number is `max_rounds` in the `[assistant]` part of `prefs.toml`; there is no field for it
in the panel. With the API key, Opus, Sonnet and Fable may answer on another model when the
chosen one declines a request.

## Security

With your Claude login:

- **Claude Code's own tools are off.** Claude Code's own shell, file, web and sub-agent tools are
  turned off for the session (`Bash`, `BashOutput`, `KillShell`, `Read`, `Write`, `Edit`,
  `MultiEdit`, `NotebookEdit`, `Glob`, `Grep`, `WebFetch`, `WebSearch`, `Task`). When Claude Code
  asks permission for a tool, WiringCraft allows only its own tools
  (`mcp__wiringcraft-app__<tool>`) and declines anything else.
- **Local port.** The tools are served on a port bound to 127.0.0.1, so only this computer can
  connect. Claude Code reaches it through `wiringcraft-app --mcp-bridge 127.0.0.1:<port>`, which
  only copies lines between Claude Code and that port. At most 8 connections are served at once.
- **Token.** WiringCraft makes a new token each time it opens the port (once per app run, with
  the first message). A connection must send it within 10 seconds or it is closed.
- **Settings folder.** Claude Code starts in WiringCraft's own folder, `assistant` inside the
  settings folder, never in the project folder. A project you were sent could carry files such as
  `.npmrc` or `.claude/` that run code when the connector or Claude Code starts in its folder.
  The settings folder is `$XDG_CONFIG_HOME/wiringcraft`, else `%APPDATA%\wiringcraft`, else
  `~/.config/wiringcraft`.
- Claude Code is started without `ANTHROPIC_API_KEY` in its environment, so it uses the account
  it is logged in with, not a key that happens to be set.
- Tool calls that arrive when no turn is running (after Stop, or late) are refused, not run.

With an API key, WiringCraft talks to the Anthropic API itself and runs the tools in the app;
Claude Code and its connector are not started. The `run_command` tool still starts the
`wiringcraft` command-line program for the command it runs.

## Tools

These are all the tools the assistant has (`crates/wc-app/src/agent.rs`). Changes made through
them show on the canvas as they happen.

| Tool | What it does |
| --- | --- |
| `get_state` | Overview of the open project: pages, panel layouts, devices count, comments, what is shown and selected, undo, shared session and the drawing aids. |
| `get_page` | Everything on one page: symbols with tags and pin positions, wires, free drawings, underlay and comments. |
| `find_symbols` | Search the project's symbols and its library sources. |
| `find_parts` | Search the parts in the project and its library sources. |
| `get_devices` | List devices with tag, description, part, properties and where each is drawn. |
| `place_symbol` | Place a library symbol on a page, with the next free tag when none is given. |
| `add_wire` | Draw a wire between two pins or points on one page. |
| `update_symbol` | Move, rotate, mirror, retag or swap the drawing of a placed symbol. |
| `black_box` | Add or edit a black box (device box) with connection points that wires attach to. |
| `update_wire` | Change a wire's ends, bends, number, potential, cable, cross-section, colour or type. |
| `update_graphic` | Replace a free drawing by its id from `get_page`. |
| `add_graphic` | Add a line, polyline, rectangle, circle, arc, text or dimension to a page. |
| `delete_items` | Delete symbols, wires and free drawings. |
| `move_items` | Move items by an offset in mm. |
| `rotate_items` | Rotate symbols 90 degrees each. |
| `mirror_items` | Mirror symbols left to right. |
| `copy_items` | Copy items to the same or another page; copied devices get the next free tags. |
| `select_items` | Show the user something: go to its page and select it. |
| `add_page` | Add a schematic page and show it. |
| `update_page` | Change a page's number, title, sheet size, overview mark or cover-page mark. |
| `delete_page` | Delete a page (not the last one). |
| `duplicate_page` | Copy a page as a new page. |
| `move_page` | Move a page one place up or down. |
| `show` | Show a page or a panel layout on screen. |
| `layers` | List the View > Layers layers, or show, hide, lock or unlock one. |
| `edit_page_json` | Replace a whole page in the page file format, for bulk edits. |
| `set_device` | Set a device's description, part, accessories and properties, or rename its tag everywhere. |
| `set_project` | Rename the project, set title-block fields and set or remove the logo. |
| `get_template` | Read the page template: columns and title-block boxes. |
| `set_template` | Change the page template's columns, title-block height and boxes, or reset it. |
| `plc_racks` | Get or replace the PLC racks and their I/O cards. |
| `jumpers` | List, add or remove jumpers between terminals of a strip. |
| `strips` | List terminal strips, move terminals, add or remove spares, set levels, or reset the order. |
| `comments` | List, add, reply to, resolve, reopen or delete review comments. |
| `macros` | List, read, insert, save or edit macros. |
| `translations` | Read and set the project's translation dictionary (`translations.toml`). |
| `number_wires` | Number every connection that has no number yet. |
| `run_check` | Check the project and return what it finds. |
| `rules` | List the check rules, replace the project's own rules, or turn a library rule off or on. |
| `get_report` | A report as a Markdown table. |
| `report_layouts` | Get, set or reset report layouts. |
| `panels` | Get, create, arrange, delete or replace cabinet panel layouts. |
| `library` | Library status, or take newer library versions. |
| `export` | Export PDF, SVG and DXF drawings, AutomationML, WAGO marking or WAGO configurator lists. |
| `versions` | Save, commit, branch, version history and page history. |
| `undo` | Undo or redo the last change. |
| `open_project` | Open a project folder, or create a new one. |
| `run_action` | Run any toolbar or menu command by name, as if the user clicked it. |
| `run_command` | Run a `wiringcraft` command-line command on the open project. |
| `todo` | Search WiringCraft's todo list, or file a problem, function request or missing item. |

The connections are in `crates/wc-agent`: `acp.rs` for Claude Code, `bridge.rs` for the tool
port and `lib.rs` for the API key.
