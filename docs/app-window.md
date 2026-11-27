# The app window

This page describes what you see in the desktop app (`wiringcraft-app`): the toolbar, the
panels on the left and right, the canvas and the status bar. The menus are listed in
[app-menus.md](app-menus.md) and the keys in [app-shortcuts.md](app-shortcuts.md).

## Starting the app

Start `wiringcraft-app` with a project folder to open it at once, or with nothing to get the
start screen. A project made with the CLI opens the same way:

```
$ wiringcraft new /tmp/x/demo --name Demo --no-git
created /tmp/x/demo
$ wiringcraft-app /tmp/x/demo
```

With no project open the window shows the start screen: **New project**, **Open folder…** and
up to six **Recent projects** (hover one to see its full path). The status bar then says
"No project open".

The window title is the project name followed by "– WiringCraft". A "•" in front of it means
there are unsaved changes.

## From top to bottom

- **Menus**: File, Edit, Insert, Page, View, Library, Versions and Help.
- **Toolbar or ribbon**: the same commands as the menus, as icons. **Layout** switches between
  the two and the choice is remembered.
- **Left panel**: pages, panel layouts and devices.
- **Canvas**: the page or panel layout you are working on.
- **Right panel**: the symbol library and the properties of what is selected.
- **Assistant** (when shown): on the far right, beside the right panel.
- **Status bar**: along the bottom.

## The toolbar

The classic toolbar is the default. Its buttons come in groups, left to right. Hovering a button
shows its name, its key in brackets when it has one, and on a second line what it does. A
button for the tool in use, or a switch that is on, is drawn highlighted. A button that cannot
be used right now (Save with nothing to save, Delete with nothing selected) is greyed out. When
the window is narrow, groups move to a second row rather than being cut off.

| Group | Button | Tooltip |
| --- | --- | --- |
| Project | New project | Create a new project folder (also a git repository) |
| | Open | Open a project folder |
| | Save | Save all changed pages |
| History | Undo | Undo the last change |
| | Redo | Redo |
| Clipboard | Cut | Cut the selection |
| | Copy | Copy the selection |
| | Paste | Paste at the pointer; devices get the next free tags |
| Draw | Select | Select and move |
| | Symbol | Place the symbol chosen in the library picker |
| | Wire | Draw a wire: click a pin or point, then the other end |
| | Black box | Draw a black box for a device without a symbol: drag its rectangle, click its edges to add connection points, then click inside it (or press Enter) to tag it |
| | Rotate | Rotate the selected symbols 90°, or the symbol being placed |
| | Mirror | Mirror the selected symbols left to right, or the symbol being placed |
| | Delete | Delete the selection |
| Graphics | Text | Write a text on the sheet: click where it goes |
| | Line | Draw a line: click both ends |
| | Rectangle | Draw a rectangle: click two corners |
| | Circle | Draw a circle: click the centre, then a point on it |
| | Arc | Draw an arc: click the centre, the start and the end (it runs clockwise) |
| | Polyline | Draw a polyline: click each point; double-click or Enter ends it, a click on the first point closes it |
| | Dimension | Draw a dimension: click both points to measure, then where the dimension line goes |
| | Revision cloud | Draw a revision cloud with the latest revision: click two corners |
| | Revision table | Place the page's revision table: click its top-left corner (Delete takes it off) |
| Pages | New page | Add a schematic page |
| | Cover page | Open the cover page and edit the project's name and fields; they show on the cover and every title block |
| | Page template | Edit the frame's reference columns and the title block's boxes, shared by every page |
| | Duplicate page | Copy this page; its devices get the next free tags |
| | Page properties | Number, title and sheet size of this page |
| | Delete page | Delete this page |
| View | Zoom in | Zoom in |
| | Zoom out | Zoom out |
| | Fit page | Show the whole page |
| | Grid | Show or hide the grid |
| | Layers | Show, hide and lock drawings, wire numbers, cross-references, comments and scanned sheets on the canvas |
| Library | Library | Compare the project's library items with the library |
| | Update | Take the newer library versions |
| | New symbol | Draw a new symbol and save it into a library folder |
| | Edit symbol | Change a library symbol (the selected one, or pick one) |
| Versions | Commit | Record a version of the project |
| | Branch | Start a branch from this version |
| | History | Show the version history as a subway map |
| Session | Share session | Let colleagues on the network work in this project with you; you let each one in |
| | Join session | Work in a colleague's shared project: pick their session and ask to join |
| Review | Comment | Pin a review comment on the sheet: click where it goes |
| | Page history | Who changed this page, when, and what they changed |
| Output | Check | Check the project: devices drawn twice, missing coils, shorts, open pins |
| | Reports | Contents, parts, devices, wires, nets, terminals, cables, PLC I/O and locations as tables |
| | PDF | Export all pages and the reports to PDF |
| Assistant | Assistant | Show the AI assistant: tell it what to draw or change and it works in the open project |

Two switches sit at the right end of the toolbar: **Dark mode** (switch between the light and
dark interface) and, at the very end, **Layout** (switch between classic toolbars and the ribbon).
The group names above are only for this page; the classic toolbar separates groups with a thin
line and shows no names.

## The ribbon

The ribbon shows the same commands as labelled buttons on tabs, with each group's name under
it. The tabs and their groups are:

- **Start**: Project (also Save as, the SVG/DXF, AutomationML and WAGO exports, and WAGO
  articles back), Edit (also Find), Assistant, Draw, Graphics.
- **Pages**: Pages (also Move page up or down, Previous and Next page), Terminals (Terminal
  strips), View (also Check while drawing).
- **Import**: Projects and pages, Parts.
- **Library**: Library, Symbols (also Import symbols).
- **Versions**: Versions, Review, Session.
- **View**: Appearance (Layout, Dark mode), Assistant.

## Left panel

- **Pages** (with a count): every page as number and title; click one to open it. Double-click
  opens its page properties, or for the cover page the cover page form. Right-click for Page
  properties, Duplicate page, Move page up, Move page down and Delete page. A page a colleague
  has open in a shared session is shown in italics with their name, and is read-only for you.
  **+ Cover page** shows when the project has no cover page.
- **Panels** (only when the project has panel layouts): click one to show it on the canvas.
  Hovering shows its size and how many devices it holds. Right-click for Arrange devices and
  Delete panel layout.
- **Devices** (with a count): every device tag in the project. Hovering shows the pages it is
  on; a click opens its first page with it selected. For a terminal, right-click offers to
  export the markers of its strip.

## Right panel

- **Symbols** (with a count): the library, with a search box. Click a symbol to start placing
  it. A filled dot means the symbol is already in the project; a symbol in italics comes from
  the library and is copied into the project when you place it. Hovering shows its id and
  description.
- **Properties**: what is selected. With nothing selected it shows the page number, title and
  how many symbols and wires it has, and the page's review comments. For one symbol it shows
  its device tag, description, rotation, position and pins, and the device's part,
  accessories and device properties. For a wire it shows its wire number, potential, cable,
  core and bends. With several items selected it says how many; for several wires a value you
  type is set on all of them. With a panel layout shown it shows the layout's name and plate
  size. On a page someone else has open the values are shown but cannot be changed.

## Assistant panel

**Assistant** (Ctrl+J) shows or hides it, and the choice is remembered. Its header has
**Settings**, **New** (start a new conversation) and **×** (hide it). You type your request at
the bottom: Enter sends, Shift+Enter starts a new line. While it works it says so and offers
**Stop**. See [assistant.md](assistant.md) for setting it up.

## Canvas

The canvas shows the open page or panel layout. On a page a colleague has open it shows
"Read-only: (name) has this page open". **Layers** opens a small window with a Show and a Lock
box for each layer: Drawings and notes, Wire numbers, Cross-references, Review comments and
Scanned sheets. A locked layer is shown, but nothing on it can be picked.

## Status bar

From left to right:

- the tool in use (Select, Place symbol, Draw wire, Text, Line, and so on), as a coloured label;
- the pointer position on the sheet, `X` and `Y` in mm (dashes when the pointer is off the canvas);
- the zoom in percent;
- the last message, such as what a command did or why it was refused.

On the right:

- **Saved** with a green dot, or **Unsaved changes** with an orange dot;
- the current git branch, when the project has one;
- with **Check while drawing** on, the check result, such as "Check: no problems" or
  "Check: 1 error, 2 warnings" (red for errors, orange for warnings, "…" while it runs).
  Click it to list the problems.
