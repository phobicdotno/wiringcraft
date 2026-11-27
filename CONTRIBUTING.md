# Contributing

## The todo list

Work is tracked as GitHub issues with the `todo` label. Add one with the "Todo" issue form or
`python tools/todo.py add "Title" --body "..."`.

Users add to the same list. The "Something is broken", "Request a function" and "Something is
missing" forms label what they send `todo`, the app's Help menu opens them, and the assistant
files them through its `todo` tool when a user asks for something WiringCraft cannot do yet or
when it hits a bug or a gap itself. Agents that finish a todo file what they found missing on the
way as new todos, so the list keeps the app growing toward what people use it for.

## Claim before you start

A todo is claimed before anyone works on it, so two people or agents never build the same thing.
A claim is a lock with a time limit: nobody else can claim the issue until it runs out.

Every agent picks a name of its own (letters, digits, `.`, `_`, `-`, `/`, starting with a letter or
digit, at most 60 characters), for example a session id.
Several agents can share one GitHub account; the name is what tells them apart. Claims are for
people with write access to the repository; the bot refuses commands from anyone else.

| Comment on the issue (first line) | Effect |
|---|---|
| `/claim <agent> [hours]` | Lock it for `<agent>`, 4 hours by default, 24 at most. Refused while someone else holds it. |
| `/renew <agent> [hours]` | Extend your own lock from now. |
| `/unclaim <agent>` | Release your own lock. |
| `/unclaim force` | Release whoever holds it (for a person clearing a stuck lock). |

The claim bot (`.github/workflows/claim.yml`) answers every command on the issue, usually within a
minute, and adds or removes the `claimed` label. If GitHub dropped that run, a later run or the
10-minute sweep answers; scheduled runs are best effort and can be late, so when `todo.py` times
out, look for the answer on the issue before claiming anything else. Only people with write access
to the repository can claim (the bot asks GitHub for the permission, not just the role). A
lock past its time is free for anyone, and the sweep posts the release. A pull request's claim only
ever lengthens the agent's lock, never shortens a longer one.

The same from the command line with `tools/todo.py`, which needs the GitHub CLI (`gh`), logged in.
`claim`, `renew`, `release` and `next` post the command and wait up to 15 minutes for the bot's
answer: exit 0 granted, 2 refused (for `next`: no free todo), 3 no answer in time. The agent name
comes from `--agent NAME` or the `WC_AGENT` environment variable; `--hours` (default 4) works with
`claim`, `renew` and `next`.

```sh
export WC_AGENT=my-agent-name
python tools/todo.py list          # open todos, and who holds which
python tools/todo.py next          # claim the oldest free todo, prints its number
python tools/todo.py claim 12 --hours 8
python tools/todo.py renew 12
python tools/todo.py release 12
```

## Sending a change

Nothing is pushed to `main`; every change is a pull request.

1. Claim the todo, then branch from the latest `main` (`area/topic`, for example `panel/step`).
2. Open the pull request early, as a draft if it is not finished. Name the issue in the title,
   `Share a session on the network (#12)`, and put `Fixes #12` and `Agent: <your agent name>` in
   the description (`Closes #12`, `Resolves #12` or `Claim #12` work too; with no `Agent:` line the
   branch name is used, if it is a valid agent name). The pull request then holds the claim until
   4 hours after its last push or edit.
   Push at least every few hours while you work, or renew.
3. Run before pushing: `cargo fmt --all` (CI checks it with `cargo fmt --all --check`),
   `cargo clippy --workspace --all-targets -- -D warnings`,
   `RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps`,
   `cargo test --workspace`. In PowerShell the docs check is
   `$env:RUSTDOCFLAGS = "-D warnings"; cargo doc --workspace --no-deps; Remove-Item Env:RUSTDOCFLAGS`.
   Some tests run the `wiringcraft` program only when `WIRINGCRAFT_CLI` points at it; CI builds it
   first (`cargo build -p wc-cli`) and sets it to the absolute path of `target/debug/wiringcraft`
   (`wiringcraft.exe` on Windows), so do the same to run them. A relative path does not work,
   because cargo runs each crate's tests from that crate's folder. CI also runs these, so run them when you touch the files:
   - `.github/scripts/claim.js`: `node --test .github/scripts/claim.test.js` (Node 22).
   - `tools/todo.py`: `python3 -m py_compile tools/todo.py`.
   - `library/symbols`: every starter symbol must come from `tools/gen_starter_symbols.py`. The
     `rm` below deletes every JSON file there: files git does not track (`??` in
     `git status --porcelain -- library/symbols`) are gone for good, and uncommitted edits to
     tracked files (` M`) are lost too, since git brings back only the committed version. So
     first commit or move out anything that command lists, or run the check in a separate
     `git worktree`. Then run
     `rm -f library/symbols/*.json`, then `python3 tools/gen_starter_symbols.py`, and check that
     `git status -- library/symbols` shows no change. This is what CI does: deleting the files
     first makes a symbol file the generator no longer writes show as deleted, which fails the check.
4. CI uses the stable Rust toolchain. It builds and tests on Linux, Windows and macOS, and the Build
   workflow makes release builds of the app and the command line for each system, to download from
   the run's page under "Artifacts". CodeRabbit reviews the pull request when you
   comment `@coderabbitai review` on it (CodeRabbit reviews small public repositories only on
   request); answer or resolve its comments, and ask again after larger pushes.
5. Merge when CI is green. `main` is protected: it takes pull requests with passing checks only.

The description says what the change is in plain words, what a user now sees or can do, how it was
checked, and whether it changes a default someone relies on.
