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

## Container lanes: a relay deck ships and streams in

A container's mount namespace is fixed at creation, so the socket sitting on the
host is not reachable from inside, and nothing deck can do as an unprivileged
user changes that. `<engine> exec` offers exactly one channel across the
boundary — its own stdio — so that is what deck uses:

```
  pane ── /tmp/deck-agent-<pid>-<n>.sock ── deck-agent-relay
                                 (container) │ stdout/stdin frames
                                             │ docker exec -i
                                             │ ssh (the host's ControlMaster)
  local $SSH_AUTH_SOCK ────── mux ───────────┘  (deck)
```

The container side is the `agent-relay` crate: a dependency-free,
`forbid(unsafe_code)` static musl binary that binds a unix socket in the
container's own filesystem and multiplexes every connection it accepts over its
stdio, where `infra/ssh/agent_relay.rs` de-multiplexes each channel onto its own
connection to the local agent. Frames are `[id: u32 BE][len: u32 BE][bytes]`,
`len == 0` closing a channel; ids are minted by the accepting (container) side,
so they cannot collide.

**deck carries that binary and streams it in.** This is the part worth being
deliberate about: the obvious cheaper design is to ask the image for an
interpreter — VS Code's Dev Containers does the equivalent, tunnelling
`vscode-ssh-auth-<uuid>.sock` over its own RPC channel from a node it installs —
but a container is someone else's filesystem, and "works only if your image has
python" is a coin flip on a `distroless`, `alpine` or slim base. Shipping the
program instead means the feature has no requirements on the image at all.

Starting a relay is three hops on the host's warm master, once per container lane
per deck run:

1. **Probe** — `uname -m` inside the container, to pick which build to send.
2. **Install** — the ~440 KB binary streamed over the exec's stdin into a
   `mktemp -d` directory of our own. The directory is chosen by trying
   `$TMPDIR`, `$HOME` and `/dev/shm` in turn and *executing a throwaway file* in
   each, so a `noexec` mount is discovered here rather than as an unexplained
   `exec` failure later. `0700` and freshly created, so another account in a
   shared container cannot plant the binary deck is about to run; written as
   `.part` and renamed, with the byte count read back, because every step of that
   chain exits 0 on a stream that ended early.
3. **Run** — `<engine> exec -i <dir>/relay <socket> ; rm -rf <dir>`. The relay
   exits when its stdin closes, and the shell that waited then removes the
   directory: the container has no other moment when deck is still around to
   clean up.

Every reattach after the first is free — the relay lives as long as the deck
process, and `live_socket` answers without touching the network.

Consequences worth knowing:

- **The host's forwarded agent is not in the path.** Keys reach the container
  from the deck process itself. `forward_agent: false` still turns it off,
  because that flag is a statement about the machine, not about the mechanism.
- **Two architectures.** `x86_64` and `aarch64`, as `uname -m` spells them.
  Anything else gets no agent and says so in the log rather than sending an ELF
  for the wrong machine.
- **Nothing is pinned inside the container.** The relay's socket path is already
  stable across reconnects — and across relay restarts, since the name is per
  deck process — so the prelude publishes it as-is rather than symlinking it
  under `$HOME/.ssh`, which many images do not let the pane's user write.
- **`-i` and never `-t`.** The exec's stdio *is* the mux, so it must stay a
  binary pipe; a pty in the middle would rewrite bytes and corrupt every signing
  request.
- **Anything in the container can use the agent** while the relay is up, exactly
  as anything on a host can while `ForwardAgent` is on. The socket is `0600`, so
  "anything" means processes running as the same user, not every account in the
  image.

`agent_sock` in a container's config entry still wins when set: it means the user
bind-mounted a socket at creation and knows where it is, so deck starts no relay.

## The shipped binary

`assets/agent-relay/deck-agent-relay-{x86_64,aarch64}-linux.gz` are committed,
gzipped, and embedded with `include_bytes!`. They are *not* built by
`cargo build`: they are Linux/musl static binaries, and requiring a musl
cross-toolchain — or even rustup — on every machine that builds deck is too much
to ask for one 440 KB helper.

```bash
scripts/build-agent-relay.sh --check                          # are they current?
scripts/build-agent-relay.sh                                  # local engine
DECK_RELAY_ENGINE="docker -H ssh://devbox" scripts/build-agent-relay.sh
```

**Ordinary builds need none of this.** `cargo build` only reads the committed
files — there is no `build.rs`, nothing is cross-compiled, and no container is
started. The engine is needed only when `crates/agent-relay` changes and the
artifacts have to be regenerated; `--check` answers whether that is the case and
needs neither an engine nor a toolchain (CI runs exactly that, so a guard that
fails in CI can always be reproduced locally).

The build itself runs in a `rust:alpine` container, so it needs no local
toolchain and works against a remote engine — which is how it is done on a Mac
with no Linux in sight. Commit what it writes, including `SOURCE.sha256`.

CI also compiles the crate for both targets, and the `test` job runs the
committed artifact matching its own architecture through the wire protocol —
byte-comparing a rebuild would be meaningless, since Rust output is not
reproducible across compiler versions.

## Debugging the relay itself

It is a normal workspace member, so the host build is the fast path — no
container, no musl:

```bash
cargo build -p agent-relay
sleep 60 | ./target/debug/deck-agent-relay /tmp/relay.sock > mux.out   # give it a stdin
./target/debug/deck-agent-relay --probe /tmp/relay.sock                # a pane's view
od -An -tx1 mux.out    # 00000001 00000005 00 00 00 01 0b — id, length, request
```

`sleep 60 |` matters: the relay exits the moment its stdin closes, which is the
whole shutdown design, and a background job's stdin is `/dev/null`.

## Debugging

`DECK_AGENT_LOG=/tmp/deck-agent.log` records the relay's own diagnostics plus
everything the relay writes to stderr (nothing is opened when it is unset):

```
[box#dev] starting relay for /tmp/deck-agent-51561-25931848.sock
[box#dev] deck-agent-relay ready
```

The relay can also answer "is this socket a live agent" from inside the
container, which is the one place the question can be asked. deck's live tests
use it through `crate::remote_tmux::ssh_agent_probe`; by hand it is:

```
deck-agent-relay --probe /tmp/deck-agent-51561-25931848.sock
agent-reply-type 12 keys 1
```

Two live tests cover what unit tests cannot, both `#[ignore]`d because they need
a reachable host, a running container and a local agent holding a key. The
container needs no mount, no agent socket of its own, no root, and nothing
installed — a stock `mongo` or `nginx` container is a fair test precisely because
there is nothing in it to borrow:

```bash
docker -H ssh://host run -d --name probe --entrypoint sleep mongo:7.0 900

# the relay: probe, install, exec, and a real IDENTITIES_ANSWER from your agent
DECK_RELAY_TEST_ID=host#probe cargo test --workspace -- --ignored relay

# the last mile: what the attach leaves a new pane holding (needs tmux in the image)
DECK_RELAY_TEST_ID=host#probe cargo test --workspace -- --ignored last_mile
```
