#!/usr/bin/env bash
# End-to-end test, as close to use as it gets: the real binary in a tmux pane,
# driven by real keys (arrows, letters, shift+tab, esc, ctrl+…) and real mouse
# clicks (SGR sequences), resized like a terminal whose font is zoomed. Slack,
# Linear and GitHub are vercel-labs/emulate servers that real curl talks to;
# claude, the keychain, the clipboard and notifications are
# tests/emulator/emulate.py over a world file. The test changes both while the
# app runs. Throwaway HOME: your own files are never read.
#
#   tests/e2e.sh [path/to/deck]     needs tmux, python3, node (npm ci --prefix tests)
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
BIN="${1:-$ROOT/target/debug/deck}"
WORK="$(mktemp -d)"
SOCKET="todo-e2e-$$"
T="tmux -L $SOCKET -f /dev/null"
failures=0

cleanup() {
  $T kill-server 2>/dev/null || true
  [ -n "${EMULATE_PID:-}" ] && kill "$EMULATE_PID" 2>/dev/null
  rm -rf "$WORK"
}
trap cleanup EXIT

export HOME="$WORK/home"
export EMULATOR_WORLD="$WORK/world.json"
export EMULATOR_LOG="$WORK/calls.jsonl"
mkdir -p "$HOME/.config/deck" "$WORK/bin"
: > "$EMULATOR_LOG"
for program in claude gh security secret-tool pbcopy wl-copy xclip osascript notify-send; do
  ln -s "$ROOT/tests/emulator/emulate.py" "$WORK/bin/$program"
done
export PATH="$WORK/bin:$PATH"
OPENED="$WORK/opened.log"
printf '#!/bin/sh\necho "$1" >> %s\n' "$OPENED" > "$WORK/opener"
chmod +x "$WORK/opener"

# Slack, Linear and GitHub on three free ports, seeded, then filled through
# their APIs: the ticket, the thread, the pull request.
EMULATE="$ROOT/tests/node_modules/.bin/emulate"
[ -x "$EMULATE" ] || { echo "emulate missing: npm ci --prefix tests"; exit 1; }
URLS="$WORK/urls.json"
PORT=$(python3 -I -c "import socket; s=socket.socket(); s.bind(('127.0.0.1', 0)); print(s.getsockname()[1])")
python3 -I "$ROOT/tests/emulator/world.py" seed "$WORK/seed.json"
"$EMULATE" start --service slack,linear,github --port "$PORT" --seed "$WORK/seed.json" > "$WORK/emulate.log" 2>&1 &
EMULATE_PID=$!
for _ in $(seq 100); do grep -q Ready "$WORK/emulate.log" && break; sleep 0.1; done
grep -q Ready "$WORK/emulate.log" || { cat "$WORK/emulate.log"; echo "emulate did not start"; exit 1; }
python3 -I - "$WORK/emulate.log" "$URLS" <<'PY'
import json, re, sys
urls = dict(re.findall(r"^\s+(slack|linear|github)\s+(http://\S+)", open(sys.argv[1]).read(), re.M))
json.dump({"slack": urls["slack"] + "/api", "linear": urls["linear"] + "/graphql", "github": urls["github"]}, open(sys.argv[2], "w"))
PY
python3 -I "$ROOT/tests/emulator/world.py" build "$EMULATOR_WORLD" "$URLS"
endpoint() { python3 -I -c "import json; print(json.load(open('$URLS'))['$1'])"; }

# clipboard_has <text>: what the app last copied contains it.
clipboard_has() {
  python3 -I -c "import json,os,sys; sys.exit(sys.argv[1] not in json.load(open(os.environ['EMULATOR_WORLD'])).get('clipboard', ''))" "$1"
}

# world <python statement on `w`>: change the world while the app runs.
world() {
  python3 -I -c "import json,os; p=os.environ['EMULATOR_WORLD']; w=json.load(open(p)); $1; json.dump(w, open(p,'w'), indent=1)"
}

CALENDAR_TOOLS='["mcp__calendar__list_events"]'
config() {
  cat > "$HOME/.config/deck/config.toml" <<TOML
skill = "daily-todo"
refresh_minutes = 30
depth = "day"
active_hours = [0, 24]
weekdays_only = false
model = ""
small_model = "haiku"
calendar_tools = $CALENDAR_TOOLS
claude_bin = "claude"
status_minutes = 2
ping = "notify"
ticket_prefixes = ["ACME"]
workdir = "$WORK"
editor = ""
opener = "$WORK/opener"

[[people]]
name = "Me"
email = ""

[[people]]
name = "Ann"
email = "ann@example.test"

[[people]]
name = "Bob"
email = ""

[links]
linear = "https://linear.app/acme/issue/"
slack = ""
gmail = "https://mail.example.test/#all/"
team_inbox = ""

[endpoints]
linear = "$(endpoint linear)"
slack = "$(endpoint slack)"
github = "$(endpoint github)"
$1
TOML
}

screen() { $T capture-pane -p -t e2e:0; }

# wait_for <text> [seconds]: poll the screen, never a fixed sleep.
wait_for() {
  local text="$1" limit="${2:-10}" i
  for ((i = 0; i < limit * 10; i++)); do
    screen | grep -qF -- "$text" && return 0
    sleep 0.1
  done
  return 1
}

# gone <text> [seconds]: the text leaves the screen.
gone() {
  local text="$1" limit="${2:-10}" i
  for ((i = 0; i < limit * 10; i++)); do
    screen | grep -qF -- "$text" || return 0
    sleep 0.1
  done
  return 1
}

# called <regex>: some emulated program was called with matching program+args.
called() {
  python3 -I -c "
import json, re, sys
calls = [json.loads(line) for line in open('$EMULATOR_LOG')]
sys.exit(0 if any(re.search(sys.argv[1], ' '.join([c['program'], *c['args']])) for c in calls) else 1)" "$1"
}

check() {
  local name="$1"; shift
  if "$@"; then
    echo "ok   $name"
  else
    echo "FAIL $name"
    screen | sed 's/^/     | /'
    failures=$((failures + 1))
  fi
}

keys() { $T send-keys -t e2e:0 "$@"; }

# A key named after a pause: tmux would glue Escape and the next key into Alt+key.
key() { keys "$1"; sleep 0.15; }

# click <text>: a left click in the middle of the first place <text> is on screen.
click() {
  local at
  at=$(screen | python3 -I -c "
import sys, unicodedata
text = sys.argv[1]
for row, line in enumerate(sys.stdin.read().split('\n')):
    col = line.find(text)
    if col >= 0:
        wide = sum(1 for ch in line[:col] if unicodedata.east_asian_width(ch) in 'WF')
        print(col + wide + len(text) // 2 + 1, row + 1)
        break" "$1")
  [ -n "$at" ] || return 1
  set -- $at
  keys -l $'\e[<0;'"$1;$2"'M'
  keys -l $'\e[<0;'"$1;$2"'m'
  sleep 0.15
}

start() {
  $T new-session -d -s e2e -x 160 -y 40 -e HOME="$HOME" -e PATH="$PATH" \
    -e EMULATOR_WORLD="$EMULATOR_WORLD" -e EMULATOR_LOG="$EMULATOR_LOG" "$@"
}

stop() {
  $T kill-server 2>/dev/null || true
}

# 0. deck alone lists its apps and starts none.
check "deck alone lists its apps" sh -c "'$BIN' | grep -q '^deck todo' && '$BIN' | grep -q '^deck mcp'"

config ""
start -e ANTHROPIC_API_KEY=sk-test-should-be-removed -e EMULATOR_FAIL= "$BIN todo; sleep 60"

# 1. No snapshot: the first refresh starts on its own and the list fills.
check "first refresh fills the list" wait_for "Acme: answer the feed question" 10
check "buckets drawn" wait_for "TODAY"
check "done item shown with what closed it" wait_for "Clean the old bucket → done by hand"
check "claude never sees the API key" python3 -I -c "
import json, sys
sys.exit(any(json.loads(l)['api_key'] for l in open('$EMULATOR_LOG') if json.loads(l)['program'] == 'claude'))"

# 2. The app's own connectors: Linear, Slack, GitHub, each through its emulated API.
check "linear state of the ticket" wait_for "ACME-1 In Progress" 10
check "linear comment by someone else" wait_for "Bob commented"
check "slack reply after the ask, by name" wait_for "Slack: Bob replied"
check "pull request with a failing check" wait_for "api#1 open · checks ✗ · approved"
check "row flags who wrote since" wait_for "↩ Bob"
[ -n "${E2E_CAPTURE:-}" ] && { $T resize-window -t e2e:0 -x 118 -y 22; sleep 1; screen > "$E2E_CAPTURE"; $T resize-window -t e2e:0 -x 160 -y 40; }
# 3. A ticket closes: r refreshes the statuses, the row says so.
python3 -I "$ROOT/tests/emulator/world.py" linear-state "$URLS" ACME-1 Done
key r
check "closed ticket shows on the row" wait_for "✓ Done" 10
python3 -I "$ROOT/tests/emulator/world.py" linear-state "$URLS" ACME-1 "In Progress"
key r
check "and reopens" gone "✓ Done" 10

# 4. Agenda: your meetings, ←→ to a coworker, an unreadable calendar says why.
check "agenda shows my meeting" wait_for "Late call" 10
key Right
check "→ shows Ann's day" wait_for "Ann demo"
key Right
check "unreadable calendar says why" wait_for "calendar not shared"
key h
check "h comes back to me" wait_for "Day · Me"
key t
check "team view lists everyone" wait_for "Team day"
key Escape
check "esc leaves the team view" wait_for "Acme: answer the feed question"

# Ann's calendar cannot be read any more: her day stays, marked with when it was read.
world "w['agenda']['people'][1] = {'name': 'Ann', 'email': 'ann@example.test', 'events': [], 'error': 'calendar tool signed out'}; w['agenda_slow'] = 3"
key r
check "agenda update is shown as such" wait_for "⟳ updating" 10
check "the day says it may change" wait_for "calendars updating: may change" 5
key Right
check "Ann's day kept" wait_for "Ann demo" 15
check "marked as an earlier read" wait_for ": calendar tool signed" 15
key h
world "del w['agenda_slow']"

# 5. Help: ? lists every action with its keys; any key closes it.
key '?'
check "help panel lists the actions" wait_for "the item's actions: open a link, Claude, ticket"
check "help shows the keys as bound" wait_for "↓ j"
key Escape
check "any key closes the help" gone "the item's actions: open a link, Claude, ticket"

# 6. Actions: ⏎ opens the item's dialog; ⏎ runs the highlighted one, ↓ moves,
# a number picks, esc closes.
key Enter
check "⏎ opens the actions dialog" wait_for "1  Open Tracker ACME-1" 5
check "it offers Claude" wait_for "Let Claude work on it"
key Enter
check "⏎ opens the first link" bash -c "sleep 0.5; grep -qx 'https://example.test/acme-1' '$OPENED'"
check "and closes the dialog" gone "Let Claude work on it" 5
key Enter
key Down
key Enter
check "↓ then ⏎ opens the second link" bash -c "sleep 0.5; grep -q 'linear.app/acme/issue/ACME-1' '$OPENED'"
key Enter
check "dialog open again" wait_for "Let Claude work on it" 5
key Escape
check "esc closes it" gone "Let Claude work on it" 5

# 7. Work, from the dialog: a background Claude named after the item, its
# command copied, the badge follows it.
key Enter
check "click on Claude in the dialog" click "Let Claude work on it"
check "background session started" wait_for "Claude works on ACME-1" 10
check "session named after the item" called '^claude --bg -n ACME-1 · Acme: answer the feed question'
check "its command copied" clipboard_has "&& claude attach 0d802d74"
check "badge shows it working" wait_for "⚙ Claude working" 15
key Enter
check "the dialog now offers its session instead" wait_for "Copy the command to open Claude's session" 5
key Escape

# 8. It needs a decision: badge, toast and a soft notification.
world "w['claude']['sessions'][0]['state'] = 'blocked'"
check "badge says it waits on you" wait_for "⏸ Claude needs you" 15
check "row says so too" wait_for "⏸ needs you"
check "toast names the item" wait_for "Claude needs you on ACME-1" 5
check "notification sent" python3 -I -c "
import json, os, sys
sys.exit(not json.load(open(os.environ['EMULATOR_WORLD'])).get('notifications'))"

# 9. Take over: the dialog copies the command that opens the session, and
# opens nothing; a click on the badge too.
world "w['clipboard'] = ''"
windows_before=$($T list-windows -t e2e | wc -l)
key Enter
check "dialog shows the session waiting" wait_for "needs you" 5
check "click copies the command" click "Copy the command to open"
check "toast says copied" wait_for "copied: cd " 5
check "clipboard holds claude attach" clipboard_has "&& claude attach 0d802d74"
check "no tmux window opened" bash -c "[ \$($T list-windows -t e2e | wc -l) = $windows_before ]"
world "w['clipboard'] = ''"
check "click on the badge copies it too" click "Claude needs you"
check "the click copied it" bash -c "for i in \$(seq 50); do python3 -I -c \"import json,os,sys; sys.exit('claude attach' not in json.load(open(os.environ['EMULATOR_WORLD'])).get('clipboard',''))\" && exit 0; sleep 0.1; done; exit 1"

# 10. Ticket: an item without one gets a draft, ⏎ creates it, the item follows.
key j
check "item without ticket says so" wait_for "no Linear ticket"
key Enter
check "click on the ticket choice" click "Draft a Linear ticket"
check "draft shown for confirmation" wait_for "Supplier: reply about the export" 10
check "nothing created before ⏎" bash -c "! grep -q 'save_issue' '$EMULATOR_LOG'"
key Enter
check "ticket created" wait_for "ACME-9999 created" 10
check "create got the drafted title" grep -q 'Supplier: reply about the export' "$EMULATOR_LOG"
check "item now carries the ticket" bash -c "grep -q '\"key\": \"ACME-9999\"' '$HOME/.cache/daily-todo/last.json'"
check "selection follows the new key" wait_for "╭ ACME-9999"

# 11. Handled where the todo cannot see it: the dialog marks it done, and
# opens it again.
key Enter
check "click on mark done" click "Mark it done"
check "the row says done by hand" wait_for "→ marked done" 5
check "the snapshot keeps it" bash -c "grep -q '\"text\": \"marked done\"' '$HOME/.cache/daily-todo/last.json'"
key Enter
check "the dialog offers to reopen" wait_for "Reopen it · now marked done" 5
check "click on reopen" click "Reopen it"
check "the row is open again" gone "→ marked done" 5
check "the snapshot too" bash -c "! grep -q 'marked done' '$HOME/.cache/daily-todo/last.json'"

# Or Claude reads its sources again, read-only, and its verdict sets it.
key Enter
check "click on check" click "Check its status with Claude"
check "done with what closed it" wait_for "→ answered Ann (mail)" 10
check "by a read-only job on the item" called '(?s)^claude -p Check whether this item.*ACME-9999.*--permission-mode dontAsk'
world "w['check'] = {'done': False, 'why': 'no reply to Ann yet'}"
key Enter
check "check again" click "Check its status with Claude"
check "still open, and why" wait_for "ACME-9999 still open: no reply to Ann yet" 10
check "the row is open again" gone "→ answered Ann (mail)" 5
world "w['check'] = {'done': False, 'why': 'Ann asked again'}"
key Enter
check "check an open item" click "Check its status with Claude"
check "it stays open, with why" wait_for "ACME-9999 still open: Ann asked again" 10

# 12. Settings: a change is saved to the config file.
key s
check "settings open" wait_for "Refresh every"
key Right
key Escape
check "refresh interval saved" grep -q '^refresh_minutes = 60' "$HOME/.config/deck/config.toml"

# 13. Claude manages the todo through the MCP server, while the app is open.
mcp() {
  python3 -I - "$BIN" "$@" <<'PY'
import json, subprocess, sys
binary, calls = sys.argv[1], sys.argv[2:]
lines = [{"jsonrpc": "2.0", "id": 0, "method": "initialize",
          "params": {"protocolVersion": "2025-06-18", "capabilities": {}, "clientInfo": {"name": "e2e"}}},
         {"jsonrpc": "2.0", "method": "notifications/initialized"}]
for index, call in enumerate(calls, 1):
    name, arguments = call.split(" ", 1)
    lines.append({"jsonrpc": "2.0", "id": index, "method": "tools/call",
                  "params": {"name": name, "arguments": json.loads(arguments)}})
answers = subprocess.run([binary, "mcp"], input="\n".join(map(json.dumps, lines)) + "\n",
                         capture_output=True, text=True, timeout=30).stdout.splitlines()
for answer in map(json.loads, answers[1:]):
    result = answer["result"]
    print(("ERROR " if result["isError"] else "") + result["content"][0]["text"])
PY
}
answers=$(mcp 'todo_add {"key": "mcp-item", "line": "Added by Claude · from the MCP server", "bucket": "now"}' \
              'todo_done {"key": "old-item", "text": "closed by Claude"}' \
              'todo_status {"key": "ACME-1"}' \
              'todo_add {"key": "mcp-item", "line": "again"}' \
              'settings_set {"ping": "bell", "status_minutes": 1}')
echo "$answers" | sed 's/^/     mcp> /'
check "mcp: item added shows in the app" wait_for "Added by Claude" 5
check "mcp: done shows crossed with its reason" wait_for "Clean the old bucket → closed by Claude" 5
check "mcp: status of an item through the connectors" grep -q "ACME-1 In Progress" <<< "$answers"
check "mcp: a second add with the same key is refused" grep -q "ERROR an item mcp-item is already in the todo" <<< "$answers"
check "mcp: settings written to the config" grep -q '^ping = "bell"' "$HOME/.config/deck/config.toml"
refreshes_before=$(grep -c '"-p", "/daily-todo' "$EMULATOR_LOG" || true)
soon=$(python3 -I -c "import datetime; print((datetime.datetime.now().astimezone() + datetime.timedelta(seconds=3)).isoformat(timespec='seconds'))")
answers=$(mcp "schedule_add {\"at\": \"$soon\", \"job\": \"refresh\", \"note\": \"e2e\"}" 'schedule_list {}')
echo "$answers" | sed 's/^/     mcp> /'
check "mcp: planned run listed" grep -q "refresh e2e" <<< "$answers"
check "mcp: the app runs it when due" wait_for "planned refresh started" 10
check "mcp: through the todo skill" bash -c "sleep 1; [ \$(grep -c '\"-p\", \"/daily-todo' '$EMULATOR_LOG') -gt $refreshes_before ]"
check "mcp: a one-off leaves the schedule" grep -q "Nothing planned." <<< "$(mcp 'schedule_list {}')"

key k
# The planned refresh rewrote the todo: start again from the first item.
key g
check "selection on the first item" wait_for "╭ ACME-1" 5

# 14. The terminal shrinks and grows (a font zoom): the layout follows.
$T resize-window -t e2e:0 -x 80 -y 40
check "narrow: the day panel gives way" gone "Day · Me" 5
check "narrow: the list stays" wait_for "Acme: answer the feed question"
check "narrow: help still in the footer" wait_for "? help"
$T resize-window -t e2e:0 -x 80 -y 14
check "short: the detail box gives way" gone "╭ ACME-1" 5
$T resize-window -t e2e:0 -x 160 -y 40
check "wide again: the day panel is back" wait_for "Day · Me" 5
keys -l $'\e[<65;10;8M'
check "wheel scrolls the list" wait_for "╭ mail-item" 5

# 15. Rebound keys, Slack not connected, outside tmux (same behaviour).
key q
stop
config '
[keys]
refresh = ["ctrl+r"]'
world "del w['tokens']['slack']; w['claude']['sessions'] = []"
start "env -u TMUX $BIN todo; sleep 60"
check "a missing token says which connector" wait_for "slack not connected" 15
refreshes_before=$(grep -c '"-p", "/daily-todo' "$EMULATOR_LOG" || true)
key r
check "r no longer bound" bash -c "sleep 0.5; [ \$(grep -c '\"-p\", \"/daily-todo' '$EMULATOR_LOG') = $refreshes_before ]"
key C-r
check "ctrl+r refreshes instead" wait_for "refreshing todo, agenda and statuses" 5
key Enter
check "outside tmux, work from the dialog" click "Let Claude work on it"
check "badge for the new session" wait_for "Claude working" 15
check "work copied the attach command" clipboard_has "&& claude attach 0d802d75"

# 16. No calendar tools: no agenda job, the todo still fills.
key q
stop
CALENDAR_TOOLS='[]'
config ""
: > "$EMULATOR_LOG"
start "$BIN todo; sleep 60"
check "no calendar tools: the todo still fills" wait_for "Acme: answer the feed question" 10
key r
check "no calendar tools: no agenda job" bash -c "sleep 1; ! grep -q 'calendar events' '$EMULATOR_LOG'"

# 17. A failing job says so and leaves the list alone.
key q
stop
rm -f "$HOME/.cache/daily-todo/last.json"
start -e EMULATOR_FAIL=refresh "$BIN todo; sleep 60"
check "failed refresh is reported" wait_for "todo refresh failed: fake refresh failure" 10

# 18. deck claude: Claude in a pane, deck's panels beside it, its dialog over it.
key q
stop
REPO="$WORK/repo"
git init -q -b feature "$REPO"
echo one > "$REPO/a.txt"
git -C "$REPO" add a.txt
git -C "$REPO" -c user.name=e2e -c user.email=e2e@example.test commit -qm one
echo two >> "$REPO/a.txt"
echo new > "$REPO/new.md"
config '
[claude.watch]
title = "Rig"
command = "printf '"'"'\\033[32mrig 1 free\\033[0m'"'"'"
every_seconds = 2'
: > "$OPENED"
start "cd $REPO && $BIN claude; echo deck exited with \$?; sleep 60"
check "claude runs in deck's pane" wait_for "emulated claude ready in repo" 10
keys -l "hello there"
key Enter
check "keys reach claude" wait_for "claude heard: hello there"
check "the session as claude registered it" wait_for "e2e chat" 5
check "changed files with their branch" wait_for "⎇ feature" 5
check "a changed file with its counts" wait_for "a.txt"
check "an untracked file" wait_for "new.md"
check "the branch's pull request first" wait_for "▶ #5 Feature: export" 5
check "what it waits for" wait_for "1 failing  review required"
check "the other one, a draft in conflict" wait_for "conflicts  draft"
check "the watched command, without its colours" wait_for "rig 1 free" 5
key C-]
check "the menu key opens deck's dialog" wait_for "Fork this conversation"
key Enter
check "fork copies the command with the session id" clipboard_has "--resume e2e5e551-0000-4000-8000-000000000001 --fork-session"
check "the dialog closed" gone "Fork this conversation" 3
key C-]
check "the dialog offers the pull request" click "Open #5 in the browser"
check "the pull request opened" bash -c "sleep 0.3; grep -qx 'https://example.test/acme/api/pull/5' '$OPENED'"
check "a click on the other one opens it" click "#3 Older idea"
check "the second opened" bash -c "sleep 0.3; grep -qx 'https://example.test/acme/api/pull/3' '$OPENED'"
key C-]
check "panels can be hidden" click "Hide the side panels"
check "panels gone" gone "Pull requests" 3
key C-]
check "and shown again" click "Show the side panels"
check "panels back" wait_for "Pull requests" 3
key C-]
check "quit from the dialog" click "Quit: Claude stops too"
check "deck and claude stopped" wait_for "deck exited with" 5

# 19. Claude ending ends deck claude.
stop
start "cd $REPO && $BIN claude; echo deck exited with \$?; sleep 60"
check "claude starts again" wait_for "emulated claude ready in repo" 10
keys -l "/exit"
key Enter
check "claude's exit ends deck with its code" wait_for "deck exited with 0" 5

echo
if [ "$failures" -gt 0 ]; then
  echo "$failures check(s) failed"
  exit 1
fi
echo "all checks passed"
