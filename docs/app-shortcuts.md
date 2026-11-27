# Desktop app shortcuts

Every keyboard shortcut and mouse gesture of the desktop app (`crates/wc-app`), grouped by where
it works. On a Mac, Ctrl means Cmd. The menus and tooltips show the same shortcuts; they come
from the command registry in `crates/wc-app/src/actions.rs`, the keys are read in
`crates/wc-app/src/main.rs` (`shortcuts`, `ctrl_shortcuts`).

While a dialog is open, none of the shortcuts below run. The single-key shortcuts also wait
while a text field has the keyboard, so typing an S in a field does not start the symbol tool.

## Project and file

| Keys | What it does |
| --- | --- |
| Ctrl+Shift+N | New project |
| Ctrl+O | Open a project folder |
| Ctrl+S | Save all changed pages |
| Ctrl+Shift+S | Save as: copy the project to a new folder and go on in the copy |
| Ctrl+P | Export all pages and the reports to PDF |
| Ctrl+K | Commit: record a version |
| Ctrl+E | Check the project |
| Ctrl+R | Reports |
| Ctrl+F | Find and replace |
| Ctrl+J | Show or hide the assistant |

## Editing

| Keys | What it does |
| --- | --- |
| Ctrl+Z | Undo |
| Ctrl+Y or Ctrl+Shift+Z | Redo |
| Ctrl+C, Ctrl+X, Ctrl+V | Copy, cut, paste |
| Ctrl+D | Duplicate the selection one snap step down and right |
| Ctrl+A | Select everything on the page |
| Del or Backspace | Delete the selection |
| R | Rotate the selected symbols 90°, or the symbol being placed |
| M | Mirror the selected symbols left to right, or the symbol being placed |
| Arrow keys | Move the selection one snap step (2.5 mm by default), one undo step a press |

On a schematic page, arrow keys with Ctrl, Shift or Alt held do not move anything. The symbol
editor does not check these keys: there an arrow moves the selection with or without them.

## Tools

| Keys | Tool |
| --- | --- |
| Esc | Select (see below) |
| S | Place the symbol chosen in the library list |
| W | Draw a wire |
| T | Text |
| L | Line |
| C | Comment |

The other drawing tools (rectangle, circle, arc, polyline, dimension, revision cloud, revision
table, black box) have no key; pick them in the toolbar or ribbon.

Esc does one thing at a time: it cancels a drag in progress and puts the items back, else drops
the black box, shape or wire being drawn, else, with the Select tool, clears the selection. When
there is nothing left to drop, it goes back to the Select tool. So with another tool and nothing
being drawn, the first Esc switches to Select and keeps the selection, and a second Esc clears it.

## Pages and view

| Keys | What it does |
| --- | --- |
| Ctrl+N | Add a schematic page |
| Page Up | Previous page |
| Page Down | Next page |
| + or = | Zoom in (also with Shift held) |
| - | Zoom out |
| F | Fit the page |
| G | Show or hide the grid |

## Mouse on a schematic page

| Gesture | What it does |
| --- | --- |
| Wheel or trackpad pinch | Zoom around the pointer |
| Right or middle drag | Pan |
| Click (Select tool) | Select the item under the pointer; on empty paper, clear the selection |
| Shift+click or Ctrl+click | Add the item to the selection, or take it out |
| Drag an item | Move the selection in whole snap steps |
| Drag on empty paper | Select everything in the box; with Shift or Ctrl held, add it to the selection |
| Alt held when a move ends | Do not auto-wire pins that now line up |
| Right-click an item | Select it and open its menu: rotate, mirror, copy, cut, duplicate and delete; for wires also the route style |
| Click a cross-reference | Jump to the page it points to |
| Click a comment bubble | Open the comment in the properties panel |
| Drag a corner of a selected black box | Resize it |
| Drag the project logo | Move it; drag its bottom-right corner to resize it |

## Drawing tools

| Tool | Gestures |
| --- | --- |
| Symbol | Click on the sheet to place. R and M turn and mirror it first. Alt+click places it without auto-wiring |
| Wire | Click a pin or point to start, click to add bends, click a pin to finish, or double-click a free point to end there. Esc drops it |
| Text, Comment | Click where it goes, then type it in the dialog |
| Line, Rectangle, Circle, Arc, Dimension, Revision cloud | Click the points the status line asks for. Esc drops the shape |
| Polyline | Click each point. Enter or a double-click on the last point ends it; a click on the first point closes it |
| Revision table | Click its top-left corner |
| Black box | Drag its rectangle (or click two corners), click its edges to add connection points, then click inside it or press Enter to tag it |

## Side panels

| Where | Gesture |
| --- | --- |
| Library list | Click a symbol to pick it and switch to the Symbol tool |
| Pages list | Click to open a page. Double-click opens its properties (the cover page opens its form). Right-click: cover page, properties, duplicate, move up or down, delete |
| Panels list | Click to open a panel layout. Right-click: arrange or delete it |
| Devices list | Click to show the device on its page. Right-click a terminal: export the markers of its strip |
| Property fields | Enter or leaving the field applies the value |

## Panel layout view

| Keys or gesture | What it does |
| --- | --- |
| Wheel or pinch, right or middle drag | Zoom and pan, as on a page |
| Click or right-click | Pick the device, rail or duct under the pointer |
| Drag | Move the picked item; a device near a DIN rail clips onto it. Drag a rail's or duct's handle to change its size |
| Arrow keys | Move the picked device one snap step (at most 5 mm, 1 mm with snapping off) |
| Esc during a drag | Put the item back |
| R, Del | Rotate or delete the picked item (also in its right-click menu) |
| Ctrl+Z, Ctrl+Y, Ctrl+Shift+Z | Undo and redo, in order with the arrow presses |

## Symbol editor

While the symbol editor is open, the schematic's keys are held back. These work in it:

| Keys or gesture | What it does |
| --- | --- |
| Ctrl+Z, Ctrl+Y or Ctrl+Shift+Z | Undo, redo in the editor |
| Del or Backspace | Delete the selected shape or pin |
| Arrow keys | Move the selected shape or pin one 2.5 mm step |
| Click | Place the tool's next point, or select a shape |
| Drag (Select tool) | Move a shape; drag a handle of the selected shape to reshape it |
| Double-click on the last point, or Enter | End a polyline |
| Esc | Cancel a drag, else drop the shape being drawn |
| Right-click | Drop the shape being drawn |

Cut, copy, paste, duplicate, rotate, mirror and select all are refused while it is open.

## Terminal strip editor

Drag a terminal's ≡ handle to another place on the rail, or use the ↑ and ↓ buttons.

## Dialogs

Enter confirms the one-line field of the Open, Save as, Text, Import and attach dialogs, the
black box tool's connection point and tag, and the language field of Translations. Esc cancels
the Text, Comment, connection point, WAGO marking and import options dialogs, goes back from the
black box tag, and closes the page history.

## Assistant

| Keys or gesture | What it does |
| --- | --- |
| Enter | Send the message |
| Shift+Enter | New line |
| Ctrl+V in the message box | Paste a picture from the clipboard as a screenshot (Windows and Linux; on macOS Cmd+V does not do it, use the Paste screenshot button or drop the file) |
| Drop a picture on the window | Attach it as a screenshot, while the assistant panel is shown and not working on a message (then the drop is refused with a note) |
