# ssh-agent forwarding

deck hands every remote pane a working `SSH_AUTH_SOCK` so shells and coding
agents in a remote session can use local keys without a private key ever being
copied to the remote. How it does that differs by lane kind, because the two
have different problems.

## Host lanes: `ForwardAgent` plus a stable name

`forward_agent` (per host, default on) puts `-o ForwardAgent=yes` on **every**
ssh invocation for that host — `infra/ssh/client.rs` keeps the per-host answer in
a process-wide table and `base_ssh_args` appends it. It has to be every
invocation: the master connection decides what the multiplexed sessions get
("the display and agent forwarded will be the one belonging to the master
connection", ssh_config(5)), and deck's first call to a host is normally a
one-shot listing, not the attach.

sshd then mints a *fresh* `$SSH_AUTH_SOCK` (`/tmp/ssh-*/agent.N`) per
connection, so any pane that captured an older path holds a dead socket after a
reconnect. The attach prelude (`app/ssh/remote_spawn.rs`) therefore points
`~/.ssh/deck-agent-<pid>-<nanos>.sock` at the live socket on every attach and
publishes *that* — to the attached client and to the tmux server's global
environment. The name is per deck process: a single fixed name in a shared
remote account is last-attach-wins, so two people decking into `deploy@host`
could sign with each other's keys. Every run leaves one link behind; the
prelude's sweep removes the ones whose target is gone.

## Container lanes: a relay over the exec's stdio

A container's mount namespace is fixed at creation, so the socket sitting on the
host is not reachable from inside, and nothing deck can do as an unprivileged
user changes that. `<engine> exec` offers exactly one channel across the
boundary — its own stdio — so that is what deck uses (`infra/ssh/agent_relay.rs`):

```
  pane ── /tmp/deck-agent-<pid>-<n>.sock ── relay payload
                                 (container) │ stdout/stdin frames
                                             │ docker exec -i
                                             │ ssh (the host's ControlMaster)
  local $SSH_AUTH_SOCK ────── mux ───────────┘  (deck)
```

One child per container lane: `ssh <host> '<engine> exec -i <name> python3 -c …'`.
Inside, the payload binds a unix socket **in the container's own filesystem** and
multiplexes every connection it accepts over its stdio; deck de-multiplexes each
channel onto its own connection to the user's local agent. Frames are
`[id: u32 BE][len: u32 BE][bytes]`, `len == 0` closing a channel; ids are minted
by the accepting (container) side, so they cannot collide.

This is the shape VS Code's Dev Containers uses for the same problem — a
`vscode-ssh-auth-<uuid>.sock` inside the container, tunneled to the client's
`$SSH_AUTH_SOCK` over its own RPC channel — and for the same reason: it is the
only path that needs neither a pre-existing bind mount, nor root on the host,
nor recreating the container.

Consequences worth knowing:

- **The host's forwarded agent is not in the path.** Keys reach the container
  from the deck process itself. `forward_agent: false` still turns it off,
  because that flag is a statement about the machine, not about the mechanism.
- **The image needs a python** (`python3`, else `python`). That is the one thing
  deck cannot supply without shipping a binary into someone's container. Without
  it the lane simply has no agent, and `DECK_AGENT_LOG` says
  `deck-agent-relay no-python`.
- **Nothing is pinned inside the container.** The relay's path is already stable
  across reconnects — and across relay restarts, since the name is per deck
  process — so the prelude publishes it as-is rather than symlinking it under
  `$HOME/.ssh`, which many images do not let the pane's user write.
- **`-i` and never `-t`.** The exec's stdio *is* the mux, so it must stay a
  binary pipe; a pty in the middle would rewrite bytes and corrupt every signing
  request.
- **Anything in the container can use the agent** while the relay is up, exactly
  as anything on a host can while `ForwardAgent` is on.

`agent_sock` in a container's config entry still wins when set: it means the
user bind-mounted a socket at creation and knows where it is, so deck starts no
relay.

## Debugging

`DECK_AGENT_LOG=/tmp/deck-agent.log` records the relay's own diagnostics plus
everything the payload writes to stderr (nothing is opened when it is unset):

```
[box#dev] starting relay for /tmp/deck-agent-51561-25931848.sock
[box#dev] deck-agent-relay ready
```

Two live tests cover what unit tests cannot, both `#[ignore]`d because they need
a reachable host, a running container with a python in it, and a local agent
holding a key. The container needs no mount, no agent socket of its own and no
root:

```bash
docker -H ssh://host run -d --name probe --entrypoint sleep <image> 900

# the relay itself: 8 concurrent channels, 5 interleaved rounds each
DECK_RELAY_TEST_ID=host#probe cargo test --workspace -- --ignored relay

# the last mile: what the attach leaves a new pane holding
DECK_RELAY_TEST_ID=host#probe cargo test --workspace -- --ignored last_mile
```
