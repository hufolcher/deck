# deck

Terminal apps around Claude Code.

- `deck todo`: your day in one pane, a ranked todo, your agenda and your
  team's. A Claude Code skill keeps the todo fresh; the app itself checks each
  item against Linear, Slack, GitHub and your Claude sessions, and starts
  Claude on an item from its dialog.
- `deck claude [claude args…]`: Claude Code itself, with panels beside it
  (below).
- `deck mcp`: an MCP server, so Claude manages that todo.

Everything below is about `deck todo` unless it says otherwise.

```
 Wed 25 Nov · 1 now · 1 today · 0 later · 1 done   next 23:00 Late call in 9h51        updated 13:09 · next 13:39
╭ Todo ───────────────────────────────────────────────────────────────────╮╭ Day · Me ────────────────── 1/3 ╮
│ NOW                                                                     ││  Me  An  Bo                     │
│▌  1  ACME-1     Acme: answer the feed question  · client waiting  ↩ Bob ││ ● free next 23:00 Late call     │
│ TODAY                                                                   ││ ── 13:09 now ───────────────────│
│   2 ◌mail-item  Reply to the supplier about the export  · due today     ││ 14 ┊                            │
╰─────────────────────────────────────────────────────────────────────────╯╰─────────────────────────────────╯
╭ ACME-1 ────────────────────────────────────────────────────────────────────────────────────────────────────╮
│Open  Tracker ACME-1   Linear ACME-1   api#3    click one, or ⏎ for all actions                             │
│Now  ⚙ Claude working · click: copy its command · ACME-1 In Progress · Bob commented 13:14 · api#3 open     │
╰────────────────────────────────────────────────────────────────────────────────────────────────────────────╯
 ⏎ actions  → agenda  t team  r refresh  s settings  q quit  ? help
```

## How it works

- **The todo** comes from a Claude Code skill (`daily-todo` by default) that
  reads your sources and writes `~/.cache/daily-todo/last.json`. The app runs it
  headless (`claude -p /daily-todo`) every 30 minutes in active hours, and
  redraws whenever the file changes, whoever wrote it.
- **The agenda** comes from a short job that reads calendars through the MCP
  tools named in `calendar_tools`.
- **Status, without Claude.** Every 2 minutes the app asks the sources itself:
  the Linear ticket's state and last comment, who replied in the Slack thread,
  the pull request's checks and review, and every 10 seconds `claude agents`
  for sessions on the item. A row shows `⚙` Claude working, `⏸ needs you`,
  `✓ Done`, `↩ Name` someone wrote since the item was added.
- **Work on an item.** ⏎ opens the item's actions: its links, "Let Claude
  work on it", "Copy the command to open Claude's session", "Draft a Linear
  ticket", "Check its status with Claude" (a read-only job reads its threads,
  ticket and pull request again and sets it done or open), and "Mark it done"
  for what you handled where the todo cannot see it (a call), or "Reopen it".
  The next refresh keeps it done. Work starts `claude --bg` headless, with the item in the prompt; the
  session stops before anything outward and says what waits on you, and you get
  a desktop notification (`ping`) when it does. The command that opens the
  session goes to the clipboard: deck never opens a window or pane itself.
- **Claude manages the todo** through the built-in MCP server (below): it can
  add, close and move items, start work, change settings and plan its own
  runs.
- Every run uses your Claude Code login: `ANTHROPIC_API_KEY` and
  `ANTHROPIC_AUTH_TOKEN` are removed from the child's environment, so a key in
  your shell never turns a run into API billing.

## deck claude

Runs `claude` with your arguments in a pane deck draws, and keeps a column of
panels beside it (from `side_min_width` columns, 120 by default):

- **Session**: the conversation as Claude Code registers it
  (`~/.claude/sessions/<pid>.json`): working or idle, its name, since when.
- **Changes**: the branch, ahead/behind, each changed file with its `+/−`.
- **Pull requests**: yours that are open (`gh pr list --author @me`), the
  current branch's first (`▶`), each with what it waits for: failing or
  running checks, the review, conflicts, draft. A click opens it.
- **Watch**: any command's output every few seconds, e.g. what a test rig runs
  (`[claude.watch]`).

Every key goes to Claude except `ctrl+]` (`[claude] menu_key`), which opens
deck's dialog: fork the conversation (copies `claude --resume <id>
--fork-session`), open or copy a pull request's link, hide or show the panels,
send `ctrl+]` itself to Claude, quit. The wheel scrolls Claude's output. deck
ends with Claude, with its exit code.

## Install

```
cargo install --locked --git https://github.com/hufolcher/deck
claude mcp add --scope user deck -- deck mcp
```

## Connectors

Each reads a token from `DECK_<NAME>_TOKEN`, else from the keychain
(macOS Keychain, or the Secret Service with `secret-tool` on Linux). Tokens go to
curl on stdin, never on a command line. A connector without a token says so in
the detail box; the rest work.

| Connector | What it needs |
|---|---|
| Linear | a personal API key (Settings → Security & access → API keys): `security add-generic-password -a "$USER" -s deck-linear -w` |
| Slack | a user token from the app in [`slack-app.yml`](slack-app.yml) (read-only scopes): `security add-generic-password -a "$USER" -s deck-slack -w` |
| GitHub | `gh auth login`, or a token in `deck-github` |
| Claude | Claude Code on the PATH (`claude_bin`) |

On Linux: `secret-tool store --label=deck-linear service deck-linear`.

## Keys

Plain keys with standard defaults (arrows, `⏎`, `esc`, vi motions); `?` lists
them all. Rebind any action in the config:

```toml
[keys]
work = ["ctrl+w"]
next_person = ["right", "l"]
```

A key bound twice, or taken by a tmux root binding (`tmux list-keys -T root`),
is reported at start. Mouse: click a row, a link or a badge; the wheel scrolls.
The layout follows the terminal size, so a font zoom just reflows it.

## MCP server

`deck mcp` is an MCP server on stdio. It edits the same files the app
watches, so an open `deck todo` follows at once.

| Tool | |
|---|---|
| `todo_list`, `todo_status` | the items; what the connectors and sessions say about one now |
| `todo_add`, `todo_done`, `todo_reopen`, `todo_move` | change the todo |
| `work_start` | start a background Claude session on an item |
| `schedule_add`, `schedule_list`, `schedule_remove` | plan a refresh or a work session, once (`HH:MM` or RFC 3339) or every N minutes; it runs while `deck todo` is open |
| `settings_get`, `settings_set` | read and change the settings below |

## Config

`~/.config/deck/config.toml`, written with defaults on the first start,
edited by hand, by the settings popup (`s`) or by `settings_set`; the app reloads
it on change.

| Key | Default | |
|---|---|---|
| `skill` | `daily-todo` | the skill that writes the todo |
| `refresh_minutes` | 30 | 0 turns the refresh off |
| `active_hours`, `weekdays_only` | [8, 20], true | when the refresh runs |
| `depth` | `day` | how far back the skill looks |
| `status_minutes` | 2 | connector polling; 0 turns it off |
| `ping` | `notify` | `notify`, `bell` or `off` |
| `model`, `small_model` | | the refresh's model, the short jobs' model |
| `calendar_tools` | none | MCP tools the agenda job may call; none: no agenda job, the day panels show the todo's meeting lines |
| `ticket_prefixes` | [] | ticket ids are `<PREFIX>-<n>`; empty takes any |
| `workdir` | | where Claude runs, so its CLAUDE.md applies |
| `people` | | whose agenda to show, you first |
| `links` | | URL bases to rebuild links from ids (tracker, Slack, mail, team inbox) |
| `keys` | | rebound actions |
| `[endpoints]` | the public APIs | `linear`, `slack`, `github` base URLs: GitHub Enterprise (`https://host/api/v3`), a proxy, an emulator |
| `[claude]` `menu_key`, `side_min_width` | `ctrl+]`, 120 | `deck claude`'s dialog key; narrower terminals start with the panels hidden |
| `[claude.watch]` `title`, `command`, `every_seconds` | `Watch`, none, 10 | a command whose output gets a panel, run with `sh -c` in Claude's folder; none: no panel |

## Tests

```
cargo test                          # unit tests
npm ci --prefix tests && tests/e2e.sh   # the binary in tmux, driven by keys and mouse
```

The e2e run starts [emulate](https://github.com/vercel-labs/emulate) for Slack,
Linear and GitHub. It fills them through their own APIs (a ticket, a thread, a
pull request) and points `[endpoints]` at them, so real curl does the talking.
What no emulator covers (`claude`, its interactive chat included, `gh pr list`, the keychain, the clipboard, notifications)
is `tests/emulator/emulate.py` on the PATH, over a world file. The test drives
the real binary with tmux `send-keys`, SGR mouse sequences and window resizes,
changes both worlds while it runs, then reads the screen. No account and no
network.

## License

MIT, see [LICENSE](LICENSE).
