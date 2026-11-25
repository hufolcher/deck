#!/usr/bin/env python3
"""The programs deck calls that no third-party emulator covers, over one
world file, for the e2e test. Slack, Linear and GitHub are vercel-labs/emulate
servers instead (tests/emulator/world.py fills them).

Linked under the names the app runs (claude, security, secret-tool, pbcopy,
wl-copy, xclip, osascript, notify-send), it answers like the real one:
claude the headless jobs, `agents --json --all`, `--bg`, `attach` and
`mcp get|add`; the keychain the world's tokens.

  EMULATOR_WORLD  the world (JSON): tokens, claude, agenda, snapshot, clipboard, notifications
  EMULATOR_LOG    one JSON line per call: program, args, stdin, api_key
  EMULATOR_FAIL   a claude job (refresh|agenda|draft|create) that fails

The test edits the world while the app runs: a session starts waiting on you,
a token goes away.
"""
import json
import os
import re
import sys
import time
import urllib.parse

WORLD = os.environ.get("EMULATOR_WORLD", "")
LOG = os.environ.get("EMULATOR_LOG", "/dev/null")


def load():
    with open(WORLD) as f:
        return json.load(f)


def save(world):
    tmp = WORLD + ".tmp"
    with open(tmp, "w") as f:
        json.dump(world, f, indent=1)
    os.replace(tmp, WORLD)


def out(value):
    print(value if isinstance(value, str) else json.dumps(value))


def fail(message, code=1):
    print(message, file=sys.stderr)
    sys.exit(code)


def now_iso():
    stamp = time.strftime("%Y-%m-%dT%H:%M%z")
    return stamp[:-2] + ":" + stamp[-2:]


# --- curl: Linear and Slack ---------------------------------------------------

def keychain(args):
    service = args[args.index("-s") + 1] if "-s" in args else args[-1]
    token = load().get("tokens", {}).get(service.removeprefix("deck-"))
    if token is None:
        fail("security: SecKeychainSearchCopyNext: The specified item could not be found in the keychain.", 44)
    out(token)


def clipboard(stdin):
    world = load()
    world["clipboard"] = stdin
    save(world)


def notify(program, args):
    world = load()
    world.setdefault("notifications", []).append(" ".join([program, *args]))
    save(world)


# --- claude ---------------------------------------------------------------------

def claude(args):
    if args[:1] == ["agents"]:
        return out(load().get("claude", {}).get("sessions", []))
    if args[:1] == ["attach"]:
        print(f"attached {args[1]}", flush=True)
        time.sleep(60)
        return
    if args[:2] == ["mcp", "get"]:
        servers = load().get("claude", {}).get("mcp", {})
        if args[2] not in servers:
            fail(f"No MCP server found with name: {args[2]}")
        return out(f"{args[2]}: {' '.join(servers[args[2]])}")
    if args[:2] == ["mcp", "add"]:
        world = load()
        name = next(arg for arg in args[2:] if not arg.startswith("-") and arg != "user")
        world.setdefault("claude", {}).setdefault("mcp", {})[name] = args[args.index("--") + 1:]
        save(world)
        return out(f"Added stdio MCP server {name}")
    if "--bg" in args:
        world = load()
        state = world.setdefault("claude", {})
        short = state.get("next_id", "0d802d74")
        name = args[args.index("-n") + 1]
        state.setdefault("sessions", []).append({
            "kind": "background", "id": short, "sessionId": f"{short}-0000-4000-8000-000000000000",
            "name": name, "cwd": os.getcwd(), "startedAt": int(time.time() * 1000), "state": "working",
        })
        state["next_id"] = "%08x" % (int(short, 16) + 1)
        save(world)
        print(f"backgrounded · {short} · {name}")
        print(f"  claude attach {short}    open in this terminal")
        return
    if "-p" in args:
        return job(args[args.index("-p") + 1])
    fail("claude emulator: unknown call " + " ".join(args))


def job(prompt):
    kind = ("refresh" if prompt.startswith("/daily-todo") else
            "agenda" if "calendar events" in prompt else
            "draft" if "Draft one" in prompt else
            "create" if "Create it with one save_issue" in prompt else "unknown")
    time.sleep(float(os.environ.get("EMULATOR_DELAY", "0")))
    if os.environ.get("EMULATOR_FAIL") == kind:
        out({"is_error": True, "result": f"fake {kind} failure"})
        sys.exit(1)
    if kind == "refresh":
        snapshot = load().get("snapshot", {})
        snapshot["at"] = now_iso()
        for item in snapshot.get("items", []):
            item.setdefault("added_at", snapshot["at"])
        path = os.path.expanduser("~/.cache/daily-todo/last.json")
        os.makedirs(os.path.dirname(path), exist_ok=True)
        with open(path, "w") as f:
            json.dump(snapshot, f, indent=1)
        return out({"is_error": False, "result": f'{len(snapshot.get("items", []))} items'})
    if kind == "agenda":
        time.sleep(load().get("agenda_slow", 0))
        return out({"is_error": False, "result": "", "structured_output": load().get("agenda", {"people": []})})
    if kind == "draft":
        return out({"is_error": False, "result": "", "structured_output": {
            "title": "Supplier: reply about the export",
            "description": "The supplier asked about the export on Monday.\nSource: mail thread.",
            "team": "Core", "state": "Todo", "priority": 2, "project": None,
            "links": [{"title": "Mail", "url": "https://example.test/mail"}], "possible_duplicate": None}})
    if kind == "create":
        return out({"is_error": False, "result": "", "structured_output": {
            "identifier": "ACME-9999", "url": "https://linear.app/acme/issue/ACME-9999/fake"}})
    out({"is_error": True, "result": "claude emulator: unknown job"})
    sys.exit(1)


def main():
    program = os.path.basename(sys.argv[0])
    args = sys.argv[1:]
    # Only the programs fed on stdin read it: the others may run from a shell
    # whose stdin never closes.
    takes_stdin = program in ("pbcopy", "wl-copy", "xclip")
    stdin = sys.stdin.read() if takes_stdin and not sys.stdin.isatty() else ""
    with open(LOG, "a") as f:
        f.write(json.dumps({"program": program, "args": args, "stdin": stdin,
                            "api_key": bool(os.environ.get("ANTHROPIC_API_KEY"))}) + "\n")
    if program == "claude":
        claude(args)
    elif program in ("security", "secret-tool"):
        keychain(args)
    elif program in ("pbcopy", "wl-copy", "xclip"):
        clipboard(stdin)
    elif program in ("osascript", "notify-send"):
        notify(program, args)
    else:
        fail(f"emulator: {program} is not emulated")


if __name__ == "__main__":
    main()
