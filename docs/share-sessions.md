# Shared sessions

Several people can work in one open project at the same time over the office network. One
WiringCraft window shares the project (the host); the others join it (guests). The host's copy
decides everything. Each guest keeps a copy of the project on their own disk and only changes
travel over the network after joining.

The code is in `crates/wc-share` (the session rules, transport and discovery) and
`crates/wc-app/src/share.rs` (the windows and the sync with the open project).

## Where to find it

**Share session** and **Join session** are under **Versions** in the menu (next to Commit,
Branch and History), in the classic toolbar, and in the **Session** group of the ribbon's
Versions tab.

## Sharing a project

1. Open the project and pick **Share session**.
2. Check **Your name** and **Email**. They start as `git config user.name` and
   `git config user.email` (or the `WIRINGCRAFT_USER` and `WIRINGCRAFT_EMAIL` environment
   variables when set). They are the author of your changes in the history.
3. Pick **Start sharing**.

Starting a session saves the project first if it has unsaved changes. The project folder is made
a git repository if it is not one yet. The **Shared session** window then shows
"Sharing. Others join at `<address>:<port>`." Give that address to anyone whose network does not
pass the announcement (see Discovery below).

The project's vendored symbol and part files and its pictures (the logo, page underlays) are sent
to each guest that joins. Files that cannot be read are listed in a note, and guests without the
libraries cannot draw them.

**Stop sharing** in the session window ends the session for everyone.

## Joining a session

1. Pick **Join session**. The window lists the sessions heard on this network as
   `project · host (address)`. A session that runs another session protocol version is shown
   but cannot be picked; the row says which versions differ.
2. Pick a session, or type the host's address in **Address** (for example
   `192.168.1.20:47812`). An address without a port gets port 47812.
3. Check **Your name**, **Email** and **Keep my copy in** (by default a `Shared` folder in the
   WiringCraft projects folder).
4. Pick **Ask to join**. The window says "Waiting for the host at ... to let you in."

Once the host lets you in, the project is written to a new folder under **Keep my copy in**,
named after the project (`name (2)`, `name (3)` and so on when the name is taken, so an earlier
copy is never written over), and opened there. A symbol the host did not send is taken from your
own libraries only in the exact version the session pinned; otherwise it is reported as missing
in the log.

**Leave** in the session window leaves the session.

## Join approval

Every join request shows on the host's screen as "`<name>` wants to join" with the email, and
two buttons: **Let in** and **Decline**. Nobody gets in without the host's answer. Until then the
host refuses everything a guest sends.

A guest whose session protocol version differs from the host's is turned away at once, before
the host sees a request. The message says which side to update.

Names are cut to 100 characters and emails to 200, and control characters, `<` and `>` are
removed, so they always fit a git author line. An empty name becomes "Guest" and an empty email
`guest@localhost`.

## Page locks

The page a person has open is locked to them. Everyone else sees it read-only; a change made
to it anyway is put back. Opening another page, closing the page, or leaving releases the lock.
The session window lists each participant with the page they have open.

Project-level data goes through the host one change at a time: devices, comments, cabinet
panels, project properties, adding, removing and renumbering pages, and symbols, parts and
pictures added to the project. A page's display number is project-level too, so a page edit
never changes it. A page someone else has open cannot be removed. A tool that changes several
pages nobody else has open (wire numbering, a tag renamed on every page) sends them as one
change; it is refused as a whole when someone else has one of those pages open.

When two people change the same device, comments, panel or project properties, the second
change is refused if the record changed on the host after that person's copy last had it, and
their copy gets the host's version back.

## One commit per change

Each change the host applies is saved into the host's project folder and committed with the
person who made it as the git author (`Name <email>`). Commit messages describe the change, for
example `Edit page 3 Motor feed`, `Change device -K1` or `Renumber 4 pages`.

Nothing is sent while a mouse button is down or a live form is open, so a drag over many frames
is one change and one commit.

If a change cannot be committed, the host undoes it and the author is told that the change
could not be recorded.

## Undo in a session

Undo takes back only your own last change, and only the parts that change touched, so other
people's work stays. Redo takes back your last undo. The session window's **Changes** list shows
every change with who made it; **Undo this change** takes back any one of them, whoever made it,
as long as nobody changed the same parts since. Each undo is a commit of its own. The list keeps
the last 500 changes.

## Network and discovery

- **Session connection: TCP port 47812.** The host listens on all interfaces (`0.0.0.0`). When
  47812 is taken (for example a second session on the same computer), any free port is used and
  the session window shows it.
- **Discovery: UDP port 47811.** While a session runs, the host sends a small announcement to
  the local broadcast address every 2 seconds. The Join session window listens on that port and
  drops a session it has not heard from for 7 seconds. Nothing is announced unless a session is
  shared, and the announcement stops when sharing stops or the app closes. Networks that do not
  pass broadcasts (most VPNs) find nothing; type the host's address instead.
- **What is announced:** the project's name, the host's name (the name on the share form), the
  session's TCP port and the session protocol version, as one JSON object.
- **What the connection carries:** one JSON object per line. On joining, the guest sends its
  name, email and protocol version; once let in, it gets the whole project (pages, devices,
  lockfile, panels, comments, project properties), the vendored symbol and part files, the
  pictures, the current locks, the participants and the change list. After that only changes,
  locks and the participant list travel.

### Limits

- The connection is plain TCP. The code does not encrypt it and asks for no password: the host's
  approval of each join request is the only check. Use it on a network you trust.
- A line longer than 64 MiB, or one that is not a valid session message, closes that connection
  only.
- One picture can be at most 16 MiB. Participants can add at most 200 pictures to a session, and
  256 MiB of pictures together, beyond those the host shared.
- An announcement is at most 2112 bytes; a very long project or host name stops the session from
  being announced, and the host is told to use a shorter name. Others can still join by address.
- A joining window lists at most 256 sessions at once.
- Connecting to a host gives up after 5 seconds per address. A peer that stops reading for 30
  seconds is dropped.

## When someone disconnects

- **A guest leaves or loses the connection.** The host releases that guest's page locks and tells
  everyone else. The host's log records whether the connection ended in order or why it failed
  (for example a write that timed out).
- **The host stops sharing.** Every guest is told the session has ended ("The shared session has
  ended."), and every lock is released.
- **The connection to the host drops without the host saying so.** The guest sees "The
  connection to the host closed." or "The connection to the host failed: `<reason>`." and the
  session ends on their side.
- **The host declines a request.** The guest sees "The host declined your request to join."

A guest's copy stays in its folder after the session ends.
