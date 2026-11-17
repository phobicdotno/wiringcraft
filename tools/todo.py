#!/usr/bin/env python3
"""The todo list from the command line, for people and agents. Needs the GitHub CLI (`gh`), logged in.

    python tools/todo.py list                  open todos and who holds them
    python tools/todo.py next  --agent NAME    claim the oldest free todo, print its number
    python tools/todo.py claim N --agent NAME [--hours H]
    python tools/todo.py renew N --agent NAME [--hours H]
    python tools/todo.py release N --agent NAME
    python tools/todo.py add "Title" [--body TEXT]

`--agent` defaults to the WC_AGENT environment variable. claim/renew/release post the command on
the issue and wait for the claim bot's answer: exit 0 when granted, 2 when refused, 3 when the bot
did not answer in time. The lock rules are in .github/scripts/claim.js.
"""
import argparse
import json
import os
import re
import subprocess
import sys
import time
from datetime import datetime, timezone

MARK = re.compile(r"<!-- wc-claim (\{.*?\}) -->")
WAIT_S = 240


def gh(*args, data=None):
    out = subprocess.run(["gh", *args], input=data, capture_output=True, text=True, encoding="utf-8")
    if out.returncode != 0:
        sys.exit(f"gh {' '.join(args[:3])}: {out.stderr.strip()}")
    return out.stdout


def repo():
    return json.loads(gh("repo", "view", "--json", "nameWithOwner"))["nameWithOwner"]


def comments(r, n):
    pages = json.loads(gh("api", "--paginate", "--slurp", f"repos/{r}/issues/{n}/comments?per_page=100"))
    return [c for page in pages for c in page]


def state(cs):
    """The newest claim state from the bot's comments: (agent, until)."""
    for c in reversed(cs):
        if c["user"]["type"] != "Bot":
            continue
        m = MARK.search(c["body"] or "")
        if m:
            d = json.loads(m.group(1))
            if "agent" in d:
                return d.get("agent"), d.get("until")
    return None, None


def active(agent, until):
    if not agent or not until:
        return False
    return datetime.now(timezone.utc) < datetime.fromisoformat(until.replace("Z", "+00:00"))


def todos(r):
    return json.loads(gh("issue", "list", "-R", r, "--label", "todo", "--state", "open", "--limit", "200",
                         "--json", "number,title,labels"))


def command(r, n, text):
    """Post a command and wait for the bot's reply to it. Returns (ok, message)."""
    posted = json.loads(gh("api", f"repos/{r}/issues/{n}/comments", "-f", f"body={text}"))
    deadline = time.time() + WAIT_S
    while time.time() < deadline:
        time.sleep(5)
        for c in comments(r, n):
            m = MARK.search(c["body"] or "") if c["user"]["type"] == "Bot" else None
            if m and json.loads(m.group(1)).get("reply_to") == posted["id"]:
                return json.loads(m.group(1)).get("ok", False), MARK.sub("", c["body"]).strip()
    return None, f"no answer from the claim bot within {WAIT_S} s (see the Claim workflow runs)"


def finish(ok, msg):
    print(msg)
    sys.exit(0 if ok else (3 if ok is None else 2))


def main():
    p = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    sub = p.add_subparsers(dest="cmd", required=True)
    sub.add_parser("list")
    for name in ("next", "claim", "renew", "release"):
        s = sub.add_parser(name)
        if name != "next":
            s.add_argument("issue", type=int)
        s.add_argument("--agent", default=os.environ.get("WC_AGENT"))
        if name in ("next", "claim", "renew"):
            s.add_argument("--hours", type=int, default=4)
    a = sub.add_parser("add")
    a.add_argument("title")
    a.add_argument("--body", default="")
    args = p.parse_args()
    r = repo()

    if args.cmd == "list":
        for t in todos(r):
            held = ""
            if any(l["name"] == "claimed" for l in t["labels"]):
                agent, until = state(comments(r, t["number"]))
                if active(agent, until):
                    held = f"  [locked: {agent} until {until[:16].replace('T', ' ')} UTC]"
            print(f"#{t['number']:<5} {t['title']}{held}")
        return
    if args.cmd == "add":
        print(gh("issue", "create", "-R", r, "--label", "todo", "--title", args.title, "--body", args.body or args.title).strip())
        return
    if not args.agent:
        sys.exit("give --agent NAME or set WC_AGENT")
    if args.cmd == "next":
        for t in sorted(todos(r), key=lambda t: t["number"]):
            if any(l["name"] == "claimed" for l in t["labels"]) and active(*state(comments(r, t["number"]))):
                continue
            ok, msg = command(r, t["number"], f"/claim {args.agent} {args.hours}")
            if ok:
                print(t["number"])
                print(msg, file=sys.stderr)
                return
            if ok is None:
                # The command is still on the issue and may yet be granted: stop, never claim a second.
                print(f"#{t['number']}: {msg}; check it with `todo.py list` before claiming another", file=sys.stderr)
                sys.exit(3)
        print("no free todo", file=sys.stderr)
        sys.exit(2)
    verb = {"claim": "claim", "renew": "renew", "release": "unclaim"}[args.cmd]
    text = f"/{verb} {args.agent}" + (f" {args.hours}" if verb != "unclaim" else "")
    finish(*command(r, args.issue, text))


if __name__ == "__main__":
    main()
