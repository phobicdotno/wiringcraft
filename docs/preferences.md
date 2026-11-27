# Desktop app preferences

The desktop app keeps your own settings in one file, `prefs.toml`. It is yours, not the
project's: nothing in it goes into a project folder, a commit or a shared session. The
command line does not read it. One exception: when the variable that picks the settings folder
(see below) is empty or a relative path, the folder is taken from the folder the app was started
in, which can be a project folder. Make sure the first of `XDG_CONFIG_HOME`, `APPDATA` and `HOME`
that is set holds a full path outside every project, or leave the first two unset.

## Where the file is

The app picks its settings folder from the first of these environment variables that is set,
then adds `wiringcraft`. A variable set to an empty value counts as set. An empty or relative
value is read from the current folder: an empty `XDG_CONFIG_HOME` or `APPDATA` gives `wiringcraft`
there, and an empty `HOME` gives `.config/wiringcraft` there:

1. `XDG_CONFIG_HOME`
2. `APPDATA`
3. `HOME`, with `.config` added

| System | Usual settings folder |
|---|---|
| Linux | `~/.config/wiringcraft` (or `$XDG_CONFIG_HOME/wiringcraft`) |
| Windows | `%APPDATA%\wiringcraft` (for example `C:\Users\you\AppData\Roaming\wiringcraft`) |
| macOS | `~/.config/wiringcraft` (not `~/Library`) |

`XDG_CONFIG_HOME` wins on every system when it is set. When none of the three is set there is
no settings folder: the app starts with the defaults and saves nothing.

The same folder holds:

| Path | What it is |
|---|---|
| `prefs.toml` | the preferences on this page |
| `logs/` | the app log, `wiringcraft.log`, rotated at 1 MB with 4 old files kept |
| `assistant/` | the empty folder Claude Code starts in for the assistant, never the project folder |

## How the file is read and written

- The app reads the file once, when it starts.
- It writes the whole file again when you change a setting in the app or open a project
  (the recent list changes). The file is only rewritten when its text changes.
- A key that is missing takes its default. A key the app does not know is ignored.
- If the file is not valid TOML, or a value has the wrong type, the whole file is dropped and
  the app starts with all defaults. The next save then writes the defaults over it.
- An unknown layer name in `[layers]` is skipped; the other names still count.
- Edit the file while the app is closed: the app does not read it again, and its next save
  writes over your edit.

## Keys

| Key | Type | Default | What it does | Set in the app |
|---|---|---|---|---|
| `chrome` | `"classic"` or `"ribbon"` | `"classic"` | menus with icon toolbars, or the ribbon | View, Layout (ribbon: View tab, Appearance) |
| `theme` | `"light"` or `"dark"` | `"light"` | light or dark interface | View, Dark mode (ribbon: View tab, Appearance) |
| `show_grid` | true or false | `true` | grid dots on the page | View, Grid; toolbar; key `G` |
| `auto_wire` | true or false | `true` | draw wires between pins that line up when a symbol is placed or moved (hold Alt to skip once) | Insert, Auto-wire |
| `snap` | number, mm | `2.5` | snap step for the pointer on the canvas and for the arrow keys; also the offset of Duplicate. 0 or less turns snapping off (Duplicate then uses 2.5 mm) | file only |
| `recent` | list of folders | empty | recent projects, newest first, at most 8, kept as full paths. The first one opens when the app starts without a folder argument | filled in when you open, create or join a project; shown in Open project and on the start screen |
| `show_assistant` | true or false | `false` | the assistant panel is shown | View, Assistant; toolbar; `Ctrl+J`; the panel's `×` |
| `live_check` | true or false | `true` | check the project again shortly after each edit and mark problems on the canvas | File, Check while drawing (ribbon: Pages tab, View) |
| `[layers]` | table | everything shown and unlocked | see below | View, Layers |
| `[assistant]` | table | see below | the assistant's connection and model | the assistant panel, Settings |

### `[layers]`

| Key | Type | Default | What it does |
|---|---|---|---|
| `hidden` | list of layer names | `[]` | layers not shown on the canvas |
| `locked` | list of layer names | `[]` | layers shown, but nothing on them can be picked or clicked |

Layer names: `drawings` (drawings and notes), `wire-numbers`, `cross-references`, `comments`
(review comments) and `underlay` (scanned sheets). Only `drawings`, `cross-references` and
`comments` can be locked; a lock on the others has no effect.

### `[assistant]`

| Key | Type | Default | What it does | Set in the app |
|---|---|---|---|---|
| `backend` | `"claude_login"` or `"api_key"` | `"claude_login"` | Claude Code under your Claude login, or the Anthropic API with a key | Settings, Connect with |
| `agent_command` | text | empty (left out of the file) | the command line that starts Claude Code's ACP adapter. Empty: `claude-agent-acp` or `claude-code-acp` when found, else `npx -y @agentclientprotocol/claude-agent-acp`. Quotes keep spaces in a word; no shell is used | Settings, Command (Claude login) |
| `login_model` | text | empty (left out of the file) | the model to ask Claude Code for; empty keeps Claude Code's default | Settings, Model (Claude login, once Claude Code is running) |
| `model` | text | `"claude-opus-5-5"` | the API model: `claude-opus-5-5`, `claude-sonnet-5-5`, `claude-fable-5-2` or `claude-haiku-5-5` | Settings, Model (API key) |
| `effort` | text | `"high"` | effort for API requests: `low`, `medium`, `high`, `xhigh` or `max` | Settings, Effort (API key) |
| `api_key` | text | empty (left out of the file) | the Anthropic API key, used only when `ANTHROPIC_API_KEY` is unset or blank (only spaces). Stored as plain text | Settings, API key (hidden while `ANTHROPIC_API_KEY` is set and not blank) |
| `max_rounds` | whole number | `60` | with an API key, the most requests in one turn before the assistant stops and asks to go on. 0 counts as 1 | file only |

### Example

```toml
chrome = "classic"
theme = "dark"
show_grid = true
auto_wire = true
snap = 2.5
recent = ["/home/you/Documents/WiringCraft/Motor control"]
show_assistant = false
live_check = true

[assistant]
backend = "api_key"
model = "claude-opus-5-5"
effort = "high"
max_rounds = 100

[layers]
hidden = ["underlay"]
locked = ["drawings"]
```

## Environment variables

These are read by the desktop app (`crates/wc-app`) and its assistant (`crates/wc-agent`).

| Variable | What it does |
|---|---|
| `XDG_CONFIG_HOME`, `APPDATA`, `HOME` | pick the settings folder, in that order (see above) |
| `ANTHROPIC_API_KEY` | the API key for the assistant; wins over `api_key` in the file unless it is blank (only spaces), which is ignored |
| `WIRINGCRAFT_RENDERER` | `wgpu` starts on wgpu (Direct3D, Metal or Vulkan). Any other value uses OpenGL with no fallback. Unset: OpenGL, and the app starts again on wgpu when OpenGL cannot start |
| `WIRINGCRAFT_USER` | your name on review comments and in shared sessions. Unset or blank: `git config user.name`, then `USER`, then `USERNAME`, then `someone` |
| `WIRINGCRAFT_EMAIL` | the email in a request to join a shared session. Unset or blank: `git config user.email`, else empty |
| `WIRINGCRAFT_CLI` | the `wiringcraft` command-line program the assistant runs. Unset: the one next to the app, else `wiringcraft` on the PATH |
| `WIRINGCRAFT_DWG2DXF` | the converter for a DWG page that WiringCraft's own reader refuses, as a command with `{in}` and `{out}`. Unset: `dwg2dxf`, then the ODA File Converter |
| `CLAUDE_CONFIG_DIR` | where the assistant looks for Claude Code's `.credentials.json` to tell whether you are logged in. Unset: `~/.claude` |
| `PATH` | searched for the Claude Code adapter and `npx`, together with `~/.local/bin`, `~/.npm-global/bin`, `~/.npm-packages/bin`, `~/.volta/bin`, `~/.bun/bin`, `~/.claude/local` and `~/bin`; on Windows also `%APPDATA%\npm`, elsewhere also `/opt/homebrew/bin`, `/usr/local/bin`, `/home/linuxbrew/.linuxbrew/bin` and `/usr/bin` |
| `HOME`, `USERPROFILE` | the home folder for that search and for the credentials file; the log also writes the home folder as `~` |
| `WIRINGCRAFT_BRIDGE_TOKEN` | set by the app itself for its `--mcp-bridge` helper that Claude Code starts; not for you to set |
