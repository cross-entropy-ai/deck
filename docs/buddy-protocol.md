# The Buddy wire protocol

What the [Deck Buddy](https://github.com/Junyi-99/buddy) iPad app and deck say
to each other. Both ends are written against this file: deck's half is
`src/infra/buddy/protocol.rs` (the shapes) and `src/app/buddy.rs` (the half
that can see the sidebar); the client's is `src/protocol/messages.ts`.

One plain WebSocket, advertised over Bonjour as `_buddy._tcp` on port 8765 by
default. Every frame is a JSON object with a `type`. There is no
authentication — deck asks the user about each new address instead, and until
they allow it, **nothing but `ping` is answered or acted on**.

Unknown types and unknown fields are dropped, never rejected. A client from a
newer app degrades to "that button does nothing", not to a dropped connection,
and the same holds in reverse for a deck older than the client.

## Input: client → deck

The original protocol, and still the bulk of the traffic. It types into
whatever app is **frontmost on the Mac**, not into deck, and is synthesized on
the connection's own thread so gesture-rate mouse movement never waits for a
UI frame.

```jsonc
{"type":"key","steps":[{"key":"c","modifiers":["command"]}]}
{"type":"text","text":"hello"}
{"type":"mouse","action":"move","dx":3,"dy":-4}
{"type":"mouse","action":"click","button":"right","count":2}
```

`modifiers` accepts `command`/`cmd`, `control`/`ctrl`, `shift`,
`option`/`opt`/`alt`. `action` is one of `move`, `scroll`, `click`, `down`,
`up`; `button` is `left`, `right` or `middle` and defaults to `left`.

## Liveness

```jsonc
{"type":"ping"}   →   {"type":"pong"}
```

Answered **before** approval, and while the approval prompt is up: the client
does not consider itself connected until a reply comes back, so a server that
stayed silent until the user clicked *allow* would be declared dead before
they got the chance.

It exists as a data frame because a client that cannot observe WebSocket PONG
*control* frames has no other inbound signal — React Native's
`RCTWebSocketModule` never forwards opcode 10 to JS.

## Reading deck: `state`

```jsonc
{"type":"state"}
```

Answered with everything the sidebar is showing. Unlike input, this can only
be assembled on deck's UI thread, so the connection hands the question over and
writes the reply when it comes back — usually within a frame. The connection
keeps reading meanwhile, so pings and input are not held up behind it. Up to
four questions may be outstanding at once; past that they are dropped, on the
grounds that a client asking faster than deck changes will ask again anyway.

```jsonc
{
  "type": "state",
  "tab": "agents",
  "hosts": [
    {"lane": "tmux\u001flocal", "title": "local",     "parent": null,             "status": "ok"},
    {"lane": "tmux\u001fbox",   "title": "box",       "parent": null,             "status": "connecting"},
    {"lane": "tmux\u001fbox#db","title": "box/db",    "parent": "tmux\u001fbox",  "status": "ok"}
  ],
  "sessions": [
    {"lane": "tmux\u001flocal", "name": "deck", "dir": "~/claude/deck", "selected": true}
  ],
  "agents": [
    {"lane": "tmux\u001flocal", "kind": "claude", "session": "deck", "window": "main",
     "pane": "%3", "status": "waiting", "selected": false}
  ]
}
```

**`tab`** is the tab deck is really rendering, which is not always the one
stored: a narrow terminal has no tab bar, so a preference of `agents` still
renders the session list. `projects` is deck's own name for that list, and
`sessions` is accepted as a spelling of it when selecting.

**`lane`** identifies one sidebar section — a tmux server. Treat it as opaque
and echo it back exactly: it is two parts joined by a `U+001F` unit separator,
and that is deck's business, not the client's. `title` is the human name.
`parent` names the lane a section hangs under (a container under its host),
or is `null` for a top-level one.

**Only selectable rows are listed.** A lane deck cannot list sessions for
contributes no `sessions` entry at all; what it is doing instead is its
`status`:

| `status` | meaning |
| --- | --- |
| `ok` | reachable, and its sessions are in the reply |
| `connecting` | its session list hasn't arrived yet |
| `unreachable` | deck couldn't reach it |
| `no_sessions` | reached, but its tmux server has nothing to attach to |

The three lists are flat, each row naming its own `lane`, rather than nested
host → session → agent. That is what the sidebar itself is, and a client that
only wants the agent list shouldn't have to walk a tree to find it; one pass
regroups them if it does want the nesting.

**`selected`** marks the row each sidebar cursor is on — the active session and
the active agent. Sessions and agents have separate cursors, which is why each
list has its own.

**`pane`** is the tmux `%N` pane id. Session and window names churn as panes
move between them; this is the one handle that survives, and it is what a
`select` echoes back. `kind` is `claude` or `codex`; `status` is `working`,
`idle`, `waiting` or `unknown` (the traffic-light dot the sidebar draws).

## Steering deck: `select`

The other half of remote control. Input goes to the frontmost app; this moves
deck's own selection, and does exactly what clicking that row would.

```jsonc
{"type":"select","tab":"agents"}
{"type":"select","session":{"lane":"tmux\u001flocal","name":"deck"}}
{"type":"select","agent":{"lane":"tmux\u001flocal","pane":"%3"}}
```

`tab` is `projects` (or `sessions`) or `agents`.

Fire-and-forget: nothing comes back, and the client reads the result with
another `state`. Everything is resolved against the live lists rather than
trusted as an index, so a row that has gone since the `state` reply the client
is working from is a no-op, not a switch to whatever slid into its place.

Selecting a **session** implies the session tab: deck's own cursor for it only
moves while that tab is showing, so asking for a session switches the tab too.
Selecting an **agent** leaves the tab alone — switching to an agent's pane
works from either tab.

## Approval, and what it gates

The first connection from an address raises a prompt in deck, and until it is
answered the connection is read but nothing it sends is acted on. Messages
that arrive meanwhile are discarded, not buffered: replaying a minute of queued
keystrokes at the moment of approval is not what anyone means by *allow*.

`state` and `select` are gated exactly like input — the session list is the
user's, and a switch is an action. `ping` is the only exception.

A `state` question that arrives before the answer does is *held*, not dropped,
and answered the moment the connection is allowed to act. It has to be: a
client asks for state as soon as it connects, and for an already-approved
device the verdict lands a frame or two after that. Nothing else is held — a
switch the user has not yet allowed, applied at the instant they do, is not
what `allow` means.

That is worth one note for client authors: a `select` sent in the same breath
as connecting can land before the verdict does, even for a device that was
approved long ago, and is then dropped. Wait for the first `pong` before
acting on the user's behalf — a `state` sent that early is safe, since it is
held.

An approved address is remembered in `buddy_approved` in
`~/.config/deck/config.yaml`; a denial lapses after a minute. While *any*
prompt is up every connection freezes, approved ones included, so that an
allowed device cannot synthesize the keystroke that admits the next one.
