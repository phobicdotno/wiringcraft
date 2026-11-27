# Rules for agents working in this repository

Read `CONTRIBUTING.md` first: it says how to claim a todo and how to send a change. The rules
below add to it and are not optional.

## Commit as the account you push with

Before the first commit of a session, set the author to the GitHub account whose token you use:

```sh
git config user.name "$(gh api user --jq .login)"
git config user.email "$(gh api user --jq '"\(.id)+\(.login)@users.noreply.github.com"')"
```

Check it with `git log -1 --format='%an <%ae>'` after the first commit. A commit with any other
author gets that author added to `main` as a co-author when the pull request is squashed.

## No attribution lines

A commit message, a pull request title or description, a comment and a merge message say what
changed and why. They carry none of these:

- a `Co-authored-by:` trailer
- a link to the session that wrote the change
- a "generated with" or "written by" footer naming a tool

`.claude/settings.json` turns these off for the tool that reads it. If your tool adds them some
other way, take them out before you push.

## Merging

`main` takes squash merges. Write the merge message yourself, so nothing is copied into `main`
that should not be there:

```sh
gh pr merge <number> --squash --subject "<pull request title> (#<number>)" --body "<what changed, then Closes #<issue>>"
```

Read the message once more for the lines above before you run it. Merge only pull requests whose
description names you as the agent: another agent's pull request may still be waiting for its own
check.

## Go easy on the GitHub API

Every agent on one account shares one hourly budget of API calls. When it is used up, nobody can
claim, review or merge until the hour is over.

- Wait for CI with one `gh pr checks <number> --watch --interval 60`, not a loop of your own.
- Post one `/claim` at a time and read the bot's answer before the next: claims posted seconds
  apart cancel each other's answer.
- Do not list every issue or pull request to find one: search for it
  (`gh pr list --search <number>`).
