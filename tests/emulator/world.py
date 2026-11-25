#!/usr/bin/env python3
"""The e2e world. Slack, Linear and GitHub are vercel-labs/emulate servers,
filled through their own APIs; the rest (claude, keychain, clipboard,
notifications) is emulate.py over a JSON world.

  world.py seed SEED.json             the emulate seed: users, channels, teams, repos, tokens
  world.py build WORLD.json URLS.json the ticket, the thread, the pull request; then WORLD.json
  world.py linear-state URLS.json KEY STATE   move a ticket, as someone would in Linear
  world.py offline WORLD.json         the same todo with no API and no token, for suites that
                                      only need the claude side (the dotfiles install check)

URLS.json: {"slack": …/api, "linear": …/graphql, "github": …}.
"""
import json
import sys
import time
import urllib.parse
import urllib.request

TOKENS = {"linear": "lin_api_e2e", "slack": "xoxp-e2e", "github": "ghp_e2e"}


def seed(path):
    people = [("me", "Me"), ("ann", "Ann"), ("bob", "Bob")]
    config = {
        "slack": {
            "team": {"name": "Acme", "domain": "acme"},
            "users": [{"name": name, "real_name": real, "profile": {"display_name": real}} for name, real in people],
            "channels": [{"name": "general"}],
            "tokens": [{"token": TOKENS["slack"], "type": "user", "user": "me"}]
            + [{"token": f"xoxp-{name}", "type": "user", "user": name} for name, _ in people[1:]],
        },
        "linear": {
            "users": [{"name": real, "email": f"{name}@example.test"} for name, real in people],
            "teams": [{"key": "ACME", "name": "Acme"}],
            "tokens": [{"token": TOKENS["linear"], "user": "me@example.test"},
                       {"token": "lin_api_bob", "user": "bob@example.test"}],
        },
        "github": {
            "users": [{"login": "me", "name": "Me"}],
            "orgs": [{"login": "acme", "name": "Acme", "members": [{"login": "me", "role": "admin"}]}],
            "repos": [{"owner": "acme", "name": "api", "auto_init": True}],
        },
    }
    with open(path, "w") as f:
        json.dump(config, f, indent=1)


def request(method, url, token, scheme="Bearer", json_body=None, form=None):
    headers = {"Authorization": f"{scheme} {token}".strip()}
    data = None
    if json_body is not None:
        data, headers["Content-Type"] = json.dumps(json_body).encode(), "application/json"
    if form is not None:
        data = urllib.parse.urlencode(form).encode()
    with urllib.request.urlopen(urllib.request.Request(url, data, headers, method=method)) as answer:
        body = answer.read().decode()
    return json.loads(body) if body else None


def slack(urls, token, method, **form):
    answer = request("POST", f"{urls['slack']}/{method}", token, form=form)
    assert answer.get("ok"), (method, answer)
    return answer


def linear(urls, query, token=TOKENS["linear"]):
    answer = request("POST", urls["linear"], token, scheme="", json_body={"query": query})
    assert not answer.get("errors"), answer
    return answer["data"]


def github(urls, method, path, body=None):
    return request(method, f"{urls['github']}/{path}", TOKENS["github"], json_body=body)


def state_id(urls, name):
    states = linear(urls, '{ workflowStates { nodes { id name team { key } } } }')["workflowStates"]["nodes"]
    return next(s["id"] for s in states if s["name"] == name and s["team"]["key"] == "ACME")


def linear_state(urls, key, name):
    linear(urls, f'mutation {{ issueUpdate(id: "{key}", input: {{ stateId: "{state_id(urls, name)}" }}) {{ success }} }}')


def build(path, urls):
    now = time.time()
    # Linear: ACME-1, mine, in progress, and Bob's comment after the item was added.
    team = linear(urls, '{ teams { nodes { id key } } }')["teams"]["nodes"]
    team_id = next(t["id"] for t in team if t["key"] == "ACME")
    me = linear(urls, "{ viewer { id } }")["viewer"]["id"]
    issue = linear(urls, f'mutation {{ issueCreate(input: {{ teamId: "{team_id}", title: "Answer the feed question", '
                         f'assigneeId: "{me}", stateId: "{state_id(urls, "In Progress")}" }}) {{ issue {{ identifier }} }} }}')
    key = issue["issueCreate"]["issue"]["identifier"]
    assert key == "ACME-1", key
    linear(urls, f'mutation {{ commentCreate(input: {{ issueId: "{key}", body: "Any news?" }}) {{ success }} }}', "lin_api_bob")

    # Slack: Ann asks in #general, Bob answers in the thread.
    channel = slack(urls, TOKENS["slack"], "conversations.list")["channels"][0]["id"]
    for token in (TOKENS["slack"], "xoxp-ann", "xoxp-bob"):
        slack(urls, token, "conversations.join", channel=channel)
    ask = slack(urls, "xoxp-ann", "chat.postMessage", channel=channel, text="Can you check the feed?")["ts"]
    slack(urls, "xoxp-bob", "chat.postMessage", channel=channel, text="Same question here", thread_ts=ask)

    # GitHub: acme/api#1, approved, with a failing check.
    base = github(urls, "GET", "repos/acme/api/git/ref/heads/main")["object"]["sha"]
    github(urls, "POST", "repos/acme/api/git/refs", {"ref": "refs/heads/feed", "sha": base})
    github(urls, "PUT", "repos/acme/api/contents/feed.txt", {"message": "feed", "content": "ZmVlZAo=", "branch": "feed"})
    pull = github(urls, "POST", "repos/acme/api/pulls", {"title": "Feed", "head": "feed", "base": "main"})
    github(urls, "POST", "repos/acme/api/check-runs",
           {"name": "ci", "head_sha": pull["head"]["sha"], "status": "completed", "conclusion": "failure"})
    github(urls, "POST", f"repos/acme/api/pulls/{pull['number']}/reviews", {"event": "APPROVE"})

    write(path, claude_world(now, key, f"{channel} thread {ask} {ask}", f"acme/api#{pull['number']}", TOKENS))


def claude_world(now, key, slack_ask, pull, tokens):
    added = time.strftime("%Y-%m-%dT%H:%M:%S+00:00", time.gmtime(now - 7200))
    world = {
        "tokens": tokens,
        "claude": {"sessions": [], "next_id": "0d802d74"},
        "agenda": {"people": [
            {"name": "Me", "email": "me@example.test", "events": [
                {"start": "09:00", "end": "09:15", "title": "Standup", "location": "Meet"},
                {"start": "23:00", "end": "23:30", "title": "Late call", "location": ""}]},
            {"name": "Ann", "email": "ann@example.test", "events": [
                {"start": "11:00", "end": "12:00", "title": "Ann demo", "location": ""}]},
            {"name": "Bob", "email": "", "events": [], "error": "calendar not shared"}]},
        "snapshot": {"meetings": ["09:00 Standup"], "items": [
            {"key": key, "bucket": "now", "line": "Acme: answer the feed question · client waiting",
             "asks": {"linear": key, "slack": slack_ask, "pr": pull},
             "links": [{"label": f"Tracker {key}", "url": "https://example.test/acme-1"}],
             "added_at": added, "done": None},
            {"key": "mail-item", "bucket": "today", "line": "Reply to the supplier about the export · due today",
             "asks": {"gmail": "1a15d9f8c90f9162"}, "done": None},
            {"key": "old-item", "bucket": "later", "line": "Clean the old bucket", "asks": {},
             "done": {"text": "done by hand"}}]},
    }
    return world


def write(path, world):
    with open(path, "w") as f:
        json.dump(world, f, indent=1)


if __name__ == "__main__":
    command, *rest = sys.argv[1:]
    if command == "seed":
        seed(rest[0])
    elif command == "build":
        build(rest[0], json.load(open(rest[1])))
    elif command == "offline":
        write(rest[0], claude_world(time.time(), "ACME-1", "C0123456789 1700000000.000100", "acme/api#1", {}))
    elif command == "linear-state":
        linear_state(json.load(open(rest[0])), rest[1], rest[2])
    else:
        sys.exit(f"world.py: unknown command {command}")
