# Contributing

## The todo list

Work is tracked as GitHub issues with the `todo` label. Add one with the "Todo" issue form or
`python tools/todo.py add "Title" --body "..."`.

## Claim before you start

A todo is claimed before anyone works on it, so two people or agents never build the same thing.
A claim is a lock with a time limit: nobody else can claim the issue until it runs out.

Every agent picks a name of its own (letters, digits, `.`, `_`, `-`, `/`), for example a session id.
Several agents can share one GitHub account; the name is what tells them apart.

| Comment on the issue (first line) | Effect |
|---|---|
| `/claim <agent> [hours]` | Lock it for `<agent>`, 4 hours by default, 24 at most. Refused while someone else holds it. |
| `/renew <agent> [hours]` | Extend your own lock from now. |
| `/unclaim <agent>` | Release your own lock. |
| `/unclaim force` | Release whoever holds it (for a person clearing a stuck lock). |

The claim bot (`.github/workflows/claim.yml`) answers every command on the issue within a minute
and adds or removes the `claimed` label. A lock past its time is free for anyone, and an hourly sweep
posts the release.

The same from the command line, which waits for the bot's answer (exit 0 granted, 2 refused):

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
   the description. The pull request then holds the claim, and every push extends it by 4 hours.
   Push at least every few hours while you work, or renew.
3. Run before pushing: `cargo fmt --all`, `cargo clippy --workspace --all-targets -- -D warnings`,
   `cargo test --workspace`.
4. CI builds and tests on Linux, Windows and macOS. CodeRabbit reviews the pull request when you
   comment `@coderabbitai review` on it (CodeRabbit reviews small public repositories only on
   request); answer or resolve its comments, and ask again after larger pushes.
5. Merge when CI is green. `main` is protected: it takes pull requests with passing checks only.

The description says what the change is in plain words, what a user now sees or can do, how it was
checked, and whether it changes a default someone relies on.
