//! The built-in tmux [`System`]: local and remote tmux servers exposed as one
//! mounted backend. Each configured remote host is a lane, plus the always-on
//! local lane. It owns the `local`/host dividers and the local-vs-remote
//! control/snapshot split; the remote divider's ssh-specific buttons and
//! `⇄N` badge are registered by `crate::ssh::divider`, not hardcoded here.

use crate::agent;
use crate::config::{Config, RemoteConfig};
use crate::geometry::{LaneActionAnchor, SectionButton};
use crate::lane::LaneId;
use crate::session::local::LocalControl;
use crate::session::remote::RemoteControl;
use crate::session::SessionControl;
use crate::{remote_tmux, tmux};

use super::{
    AttachmentEndpoint, AttachmentProvider, AttachmentRole, CatalogError, ControlCtx,
    FocusTransportProvider, LaneActionId, LaneActionProvider, LaneCapabilities,
    LaneConfigAddOutcome, LaneConfigOutcome, LaneConfigProvider, LaneMountProvider, LaneRuntime,
    LaneShellIntent, LaneSnapshot, MountCandidate, SectionDef, SessionCapabilities, SessionCatalog,
    SessionControlProvider, SnapshotCtx, SnapshotMode, SummaryTransportProvider, System,
};

/// This system's id — the `system` half of every [`LaneId`] it produces.
pub const TMUX: &str = "tmux";
/// The in-system lane name for the local tmux server.
/// The local lane's own name. Shared with the transport, which needs the same
/// spelling to recognise a lane id that means "this machine" — and which has
/// reserved it since before local containers existed, `host_lane("local")`
/// having always resolved to the local lane itself.
const LOCAL: &str = crate::remote_tmux::LOCAL_HOST;

/// Button command ids this system declares on its own dividers (the generic
/// `…` menu, on both local and remote). Remote-only buttons (reconnect,
/// forwards) live in `crate::ssh::divider::cmd`.
mod cmd {
    pub const MENU: &str = "menu";
    pub const NEW_SESSION: &str = "new-session";
}

/// The tmux backend. Configured remote definitions are backend-owned behind a
/// lock so the injected registry can be shared by the UI and refresh worker.
pub struct TmuxSystem {
    remotes: std::sync::RwLock<Vec<RemoteConfig>>,
    ssh_connection_reuse: std::sync::atomic::AtomicBool,
}

impl Default for TmuxSystem {
    fn default() -> Self {
        Self {
            remotes: std::sync::RwLock::new(Vec::new()),
            ssh_connection_reuse: std::sync::atomic::AtomicBool::new(true),
        }
    }
}

/// A [`MountCandidate`] id, which the shell round-trips without decoding: the
/// engine and the container name, since `activate`/`mount` receive only the id
/// and both need the engine discovery found.
fn mount_candidate_id(engine: &str, name: &str) -> String {
    format!("{engine}\x1f{name}")
}

fn parse_mount_candidate(id: &str) -> Option<(&str, &str)> {
    let (engine, name) = id.split_once('\x1f')?;
    (!engine.is_empty() && !name.is_empty()).then_some((engine, name))
}

impl TmuxSystem {
    /// The local tmux server's lane.
    pub fn local_lane() -> LaneId {
        LaneId::new(TMUX, LOCAL)
    }

    /// A remote host's lane. `host` is a *remote id*: a bare ssh host, or
    /// `host#container` for a container lane (see
    /// [`crate::remote_tmux::parse_remote_id`]).
    pub fn host_lane(host: &str) -> LaneId {
        LaneId::new(TMUX, host)
    }

    /// The lane for a container on a remote host.
    pub fn container_lane(host: &str, container: &str) -> LaneId {
        LaneId::new(
            TMUX,
            &crate::remote_tmux::container_remote_id(host, container),
        )
    }

    /// `None` for the local lane, `Some(remote id)` for a remote one — the
    /// `Option<&str>` host shape the rest of tmux's plumbing still speaks.
    /// The id is opaque above the transport: a bare ssh host, or
    /// `host#container` for a container lane.
    pub fn host_of(lane: &LaneId) -> Option<&str> {
        match lane.lane() {
            LOCAL => None,
            host => Some(host),
        }
    }

    /// The id whose *engine* this lane's containers would run on: the lane's own
    /// host for a remote one, and the local sentinel for the local lane, whose
    /// engine Deck runs right here.
    ///
    /// `None` for a container lane, which cannot mount further containers. This
    /// is what [`host_of`](Self::host_of) cannot answer: it says "not remote",
    /// where mounting needs "and here is who to ask".
    pub fn engine_host_of(lane: &LaneId) -> Option<&str> {
        match Self::host_of(lane) {
            None => Some(LOCAL),
            Some(remote_id) => crate::remote_tmux::parse_remote_id(remote_id)
                .container
                .is_none()
                .then_some(remote_id),
        }
    }
}

/// The canonical tmux lane for an `Option<&str>` host (`None` = local). The
/// bridge used while the shell's DTOs still carry `Option<String>` hosts:
/// per-lane stores key on the [`LaneId`] this produces.
pub fn lane(host: Option<&str>) -> LaneId {
    match host {
        None => TmuxSystem::local_lane(),
        Some(h) => TmuxSystem::host_lane(h),
    }
}

/// Config's `Option<String>` host list -> the `LaneId` set a per-lane store
/// (e.g. `collapsed_sections`) keys on.
pub fn lanes_from_hosts(hosts: &[Option<String>]) -> std::collections::HashSet<LaneId> {
    hosts.iter().map(|h| lane(h.as_deref())).collect()
}

/// Every managed remote id a config defines: each host followed by its
/// containers (`host#container`), in config order. The reload diff
/// onboards/offboards attachment lanes from these sets.
pub fn remote_ids(remotes: &[RemoteConfig]) -> Vec<String> {
    usable_remotes(remotes)
        .flat_map(|remote| {
            std::iter::once(remote.host.clone()).chain(usable_containers(remote).map(|container| {
                crate::remote_tmux::container_remote_id(&remote.host, &container.name)
            }))
        })
        .collect()
}

/// Config entries this system will actually mount, skipping any whose identity
/// cannot round-trip through a lane's remote id. `Config::validate` rejects
/// these on save and on hot-reload, but a hand-edited file reaches startup
/// unvalidated, and a bad entry there is worse than absent: an empty container
/// name produced the id `host#`, which reads back as the *host* `"host#"` — a
/// lane that polled a nonexistent destination every tick, claimed it could host
/// port forwards, and could never be removed.
fn usable_remotes(remotes: &[RemoteConfig]) -> impl Iterator<Item = &RemoteConfig> {
    remotes
        .iter()
        .filter(|remote| crate::config::validate_remote_host(&remote.host).is_ok())
}

fn usable_containers(
    remote: &RemoteConfig,
) -> impl Iterator<Item = &crate::config::ContainerConfig> {
    remote.containers.iter().filter(|container| {
        crate::config::validate_container_name(&container.name).is_ok()
            && crate::config::validate_container_engine(&container.engine).is_ok()
    })
}

/// The generic `…` divider menu button this system owns (both lanes).
fn menu_button() -> SectionButton {
    SectionButton {
        glyph: "…".to_string(),
        action: LaneActionId::from(cmd::MENU),
    }
}

fn new_session_button() -> SectionButton {
    SectionButton {
        glyph: "+".to_string(),
        action: LaneActionId::from(cmd::NEW_SESSION),
    }
}

/// Build one lane's [`SectionDef`]. The local lane is flush with a direct
/// new-session button; a remote lane takes the ssh-registered buttons (the `⇄N` forward
/// count + reconnect, from `crate::ssh::divider`), then the menu button. This
/// fn doesn't know which remote buttons exist — ssh decides.
fn section_def(remotes: &[RemoteConfig], lane: &LaneId, ssh_connection_reuse: bool) -> SectionDef {
    match TmuxSystem::host_of(lane) {
        None => SectionDef {
            lane: lane.clone(),
            title: "local".to_string(),
            parent: None,
            divider_title: None,
            buttons: vec![new_session_button()],
            top_margin: false,
            primary: true,
            session_capabilities: tmux_session_capabilities(),
            lane_capabilities: tmux_lane_capabilities(lane, ssh_connection_reuse),
        },
        Some(remote_id) => {
            // ssh registers the remote-only buttons (the ⇄N forward count,
            // reconnect); the menu button is appended last (rightmost), the
            // order the divider hit-tester zips against.
            let target = crate::remote_tmux::parse_remote_id(remote_id);
            // This system owns the config shape, so it resolves the lane's own
            // rules — a container's live nested inside its host's entry.
            let forwards = remotes
                .iter()
                .find(|remote| remote.host == target.host)
                .map(|remote| match target.container {
                    None => remote.forwards.as_slice(),
                    Some(name) => remote
                        .containers
                        .iter()
                        .find(|container| container.name == name)
                        .map_or(&[][..], |container| container.forwards.as_slice()),
                })
                .unwrap_or(&[]);
            // A local container has no ssh connection: no forward count to show
            // and nothing to reconnect. It gets what the local lane gets, plus
            // the menu that can unmount it.
            let mut buttons = if crate::remote_tmux::is_local_id(remote_id) {
                vec![new_session_button()]
            } else {
                crate::ssh::divider::divider(forwards, ssh_connection_reuse)
            };
            buttons.push(menu_button());
            // A container hangs under the host it runs on: the shell indents it
            // there and folds it away when the host folds. Its full title still
            // names the host, because everywhere *else* the label appears — an
            // overlay heading, a compact row's prefix — the divider above is not
            // there to say which machine this is.
            let (title, divider_title, parent) = match target.container {
                None => (remote_id.to_string(), None, None),
                Some(container) => (
                    format!("{}/{}", target.host, container),
                    Some(container.to_string()),
                    Some(TmuxSystem::host_lane(target.host)),
                ),
            };
            SectionDef {
                lane: lane.clone(),
                title,
                // A blank row above would read as a new machine starting. A
                // container is part of the block it sits in, so it stays flush.
                top_margin: parent.is_none(),
                parent,
                divider_title,
                buttons,
                primary: false,
                session_capabilities: tmux_session_capabilities(),
                lane_capabilities: tmux_lane_capabilities(lane, ssh_connection_reuse),
            }
        }
    }
}

fn tmux_session_capabilities() -> SessionCapabilities {
    SessionCapabilities {
        activate: true,
        rename: true,
        kill: true,
    }
}

/// Lane capabilities for one tmux lane. Port forwards ride Deck's shared
/// ControlMaster via `ssh -O`, so they exist only while connection reuse is on;
/// the local lane has no ssh connection at all.
fn tmux_lane_capabilities(lane: &LaneId, ssh_connection_reuse: bool) -> LaneCapabilities {
    // A lane with an ssh connection of its own — a host, not the local lane and
    // not a container, which rides its host's master.
    let owns_connection = TmuxSystem::host_of(lane).is_some_and(|remote_id| {
        crate::remote_tmux::parse_remote_id(remote_id)
            .container
            .is_none()
    });
    LaneCapabilities {
        create_session: true,
        reorder_sessions: true,
        actions: true,
        // Any remote lane can carry forwards, container included: the commands
        // are `ssh -O` against its *host's* master either way, and a container's
        // rules live in its own config entry. Neither the local lane nor a
        // container on it has an ssh connection anywhere in reach.
        port_forwards: ssh_connection_reuse
            && TmuxSystem::host_of(lane)
                .is_some_and(|remote_id| !crate::remote_tmux::is_local_id(remote_id)),
        // A host is a route to wherever the user names; a container is the
        // destination itself, and where it answers is this system's to find.
        forward_endpoint: if owns_connection {
            crate::system::ForwardEndpointKind::Explicit
        } else {
            crate::system::ForwardEndpointKind::Lane
        },
        // A host lane, or the local lane — whose engine Deck runs right here. A
        // container cannot mount further containers.
        mounts: TmuxSystem::engine_host_of(lane).is_some(),
    }
}

impl System for TmuxSystem {
    fn id(&self) -> &str {
        TMUX
    }

    fn configure(&self, config: &Config, remotes_cfg: &[crate::config::RemoteConfig]) {
        // Hand the transport layer the per-host ForwardAgent answer and the
        // per-container exec settings before any ssh spawns read them
        // (configure runs on startup and reload).
        crate::ssh::set_agent_forward_disabled(
            remotes_cfg
                .iter()
                .filter(|remote| !remote.forward_agent)
                .map(|remote| remote.host.clone())
                .collect(),
        );
        // The transport looks up container settings by remote id.
        crate::remote_tmux::set_container_opts(
            usable_remotes(remotes_cfg)
                .flat_map(|remote| {
                    usable_containers(remote).map(|container| {
                        (
                            crate::remote_tmux::container_remote_id(&remote.host, &container.name),
                            crate::remote_tmux::ContainerOpts {
                                engine: container.engine.clone(),
                                agent_sock: container.agent_sock.clone(),
                            },
                        )
                    })
                })
                .collect(),
        );
        let mut remotes = self
            .remotes
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        *remotes = remotes_cfg.to_vec();
        self.ssh_connection_reuse.store(
            config.ssh_connection_reuse,
            std::sync::atomic::Ordering::Relaxed,
        );
    }

    fn lanes(&self) -> Vec<LaneId> {
        let remotes = self
            .remotes
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        std::iter::once(Self::local_lane())
            .chain(usable_remotes(&remotes).flat_map(|remote| {
                // The local entry is not a host: its lane is already the one
                // above, and `host_lane(LOCAL)` *is* that lane — emitting it
                // again would duplicate the section. Only its containers are new.
                let host_lane = (remote.host != LOCAL).then(|| Self::host_lane(&remote.host));
                host_lane.into_iter().chain(
                    usable_containers(remote)
                        .map(|container| Self::container_lane(&remote.host, &container.name)),
                )
            }))
            .collect()
    }

    fn section_for(&self, lane: &LaneId) -> Option<SectionDef> {
        if lane.system() != TMUX {
            return None;
        }
        let remotes = self
            .remotes
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        Some(section_def(
            &remotes,
            lane,
            self.ssh_connection_reuse
                .load(std::sync::atomic::Ordering::Relaxed),
        ))
    }

    fn runtime(&self, lane: &LaneId) -> Option<LaneRuntime<'_>> {
        (lane.system() == TMUX).then(|| {
            LaneRuntime::new(lane)
                .with_catalog(self)
                .with_session_control(self)
                .with_lane_actions(self)
                .with_lane_config(self)
                .with_lane_mounts(self)
                .with_focus_transport(self)
                .with_summary_transport(self)
                .with_attachment(self)
                .with_capabilities(
                    tmux_session_capabilities(),
                    tmux_lane_capabilities(
                        lane,
                        self.ssh_connection_reuse
                            .load(std::sync::atomic::Ordering::Relaxed),
                    ),
                )
        })
    }
}

impl AttachmentProvider for TmuxSystem {
    fn role(&self, lane: &LaneId) -> Option<AttachmentRole> {
        (lane.system() == TMUX).then(|| {
            if Self::host_of(lane).is_none() {
                AttachmentRole::Primary
            } else {
                AttachmentRole::Managed
            }
        })
    }
}

impl FocusTransportProvider for TmuxSystem {
    fn focus_transport(
        &self,
        lane: &LaneId,
        endpoint: AttachmentEndpoint<'_>,
    ) -> Option<crate::focus::FocusTransport> {
        match (Self::host_of(lane), endpoint) {
            (None, AttachmentEndpoint::Primary { client_locator }) => {
                Some(crate::focus::FocusTransport::Local {
                    client_tty: client_locator.to_string(),
                })
            }
            (Some(host), AttachmentEndpoint::Managed { marker_id }) if marker_id > 0 => {
                Some(crate::focus::FocusTransport::Remote {
                    host: host.to_string(),
                    marker_id,
                })
            }
            _ => None,
        }
    }
}

impl SummaryTransportProvider for TmuxSystem {
    fn summary_pane(
        &self,
        lane: &LaneId,
        id: String,
        target: String,
    ) -> Option<crate::summary::SummaryPane> {
        (lane.system() == TMUX).then(|| crate::summary::SummaryPane {
            host: Self::host_of(lane).map(str::to_string),
            id,
            target,
        })
    }
}

/// The entry a container hangs under, created if this is the first one.
///
/// A remote host has an entry because linking it made one. The local lane does
/// not: it is a lane whether or not anything is mounted under it, so its entry —
/// the reserved-host one the state file writes back as `local.containers` —
/// comes into being with its first container. `None` for a host nobody linked,
/// which is a caller error rather than something to invent.
fn entry_for<'a>(remotes: &'a mut Vec<RemoteConfig>, host: &str) -> Option<&'a mut RemoteConfig> {
    if !remotes.iter().any(|remote| remote.host == host) {
        if host != LOCAL {
            return None;
        }
        remotes.push(RemoteConfig {
            host: LOCAL.to_string(),
            forwards: Vec::new(),
            forward_agent: true,
            containers: Vec::new(),
        });
    }
    remotes.iter_mut().find(|remote| remote.host == host)
}

impl LaneMountProvider for TmuxSystem {
    fn discover(&self, lane: &LaneId) -> Result<Vec<MountCandidate>, String> {
        let Some(host) = Self::engine_host_of(lane) else {
            return Ok(Vec::new());
        };
        // Hide what is already a lane. One list to consult now that a mount is
        // an ordinary entry rather than a session-scoped shadow of one.
        let existing: std::collections::HashSet<String> = {
            let remotes = self
                .remotes
                .read()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            usable_remotes(&remotes)
                .filter(|remote| remote.host == host)
                .flat_map(|remote| usable_containers(remote).map(|c| c.name.clone()))
                .collect()
        };

        Ok(crate::remote_tmux::list_containers(host)
            .into_iter()
            .filter(|found| !existing.contains(&found.name))
            .map(|found| MountCandidate {
                id: mount_candidate_id(&found.engine, &found.name),
                // The engine is worth showing: a host may run both, and it
                // decides which CLI Deck will exec through.
                label: if found.running {
                    format!("{} ({})", found.name, found.engine)
                } else {
                    format!("{} ({}, stopped)", found.name, found.engine)
                },
                needs_activation: !found.running,
            })
            .collect())
    }

    fn activate(&self, lane: &LaneId, candidate: &str) -> Result<(), String> {
        let host = Self::engine_host_of(lane).ok_or("a container cannot hold containers")?;
        let (engine, name) =
            parse_mount_candidate(candidate).ok_or("malformed container candidate")?;
        crate::remote_tmux::start_container(host, engine, name)
    }

    fn mount(
        &self,
        lane: &LaneId,
        candidate: &str,
        remotes: &mut Vec<RemoteConfig>,
    ) -> Option<LaneId> {
        let host = Self::engine_host_of(lane)?;
        let (engine, name) = parse_mount_candidate(candidate)?;
        // Publish the transport settings BEFORE returning: the shell onboards
        // the lane as soon as it has the id, and the attach spawner reads this
        // table on a worker thread.
        crate::remote_tmux::upsert_container_opts(
            crate::remote_tmux::container_remote_id(host, name),
            crate::remote_tmux::ContainerOpts {
                engine: engine.to_string(),
                agent_sock: None,
            },
        );
        let remote = entry_for(remotes, host)?;
        if !remote
            .containers
            .iter()
            .any(|container| container.name == name)
        {
            remote.containers.push(crate::config::ContainerConfig {
                name: name.to_string(),
                // Remembered because discovery, not the user, is what learned
                // it — and the transport needs it on every later run.
                engine: engine.to_string(),
                agent_sock: None,
                forwards: vec![],
            });
        }
        // Publish into this system's own list as well, so the lane exists the
        // instant the id is returned — the shell onboards it immediately, on a
        // worker thread, without waiting for the `configure` that commits the
        // caller's copy. Both writes are idempotent.
        {
            let mut own = self
                .remotes
                .write()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if let Some(remote) = entry_for(&mut own, host) {
                if !remote
                    .containers
                    .iter()
                    .any(|container| container.name == name)
                {
                    remote.containers.push(crate::config::ContainerConfig {
                        name: name.to_string(),
                        engine: engine.to_string(),
                        agent_sock: None,
                        forwards: vec![],
                    });
                }
            }
        }
        Some(Self::container_lane(host, name))
    }
}

impl LaneConfigProvider for TmuxSystem {
    fn add_lane(&self, candidate: &str, remotes: &mut Vec<RemoteConfig>) -> LaneConfigAddOutcome {
        let host = candidate.trim();
        if host.is_empty() {
            return LaneConfigAddOutcome::Invalid;
        }
        if remotes.iter().any(|remote| remote.host == host) {
            return LaneConfigAddOutcome::AlreadyExists;
        }
        remotes.push(RemoteConfig {
            host: host.to_string(),
            containers: vec![],
            forward_agent: true,
            forwards: vec![],
        });
        LaneConfigAddOutcome::Added(Self::host_lane(host))
    }

    fn remove_lane(&self, lane: &LaneId, remotes: &mut Vec<RemoteConfig>) -> LaneConfigOutcome {
        let Some(remote_id) = Self::host_of(lane) else {
            return LaneConfigOutcome::Unsupported;
        };
        // A container id names an entry *inside* its host's `containers` list,
        // not a `remotes` entry — matching it against `remote.host` would never
        // hit, leaving the divider's "Remove from list" permanently broken for
        // container lanes.
        let target = crate::remote_tmux::parse_remote_id(remote_id);
        if let Some(container) = target.container {
            let Some(remote) = remotes.iter_mut().find(|remote| remote.host == target.host) else {
                return LaneConfigOutcome::Unsupported;
            };
            let before = remote.containers.len();
            remote
                .containers
                .retain(|configured| configured.name != container);
            return if remote.containers.len() == before {
                LaneConfigOutcome::Unsupported
            } else {
                LaneConfigOutcome::Removed
            };
        }
        let before = remotes.len();
        remotes.retain(|remote| remote.host != target.host);
        if remotes.len() == before {
            LaneConfigOutcome::Unsupported
        } else {
            LaneConfigOutcome::Removed
        }
    }
}

impl SessionCatalog for TmuxSystem {
    fn snapshot(&self, lane: &LaneId, ctx: &SnapshotCtx<'_>) -> Result<LaneSnapshot, CatalogError> {
        if lane.system() != TMUX || lane.lane().is_empty() {
            return Err(CatalogError::Backend(format!(
                "invalid tmux lane routed to catalog: {}",
                lane.as_str()
            )));
        }
        match TmuxSystem::host_of(lane) {
            None => {
                let current = if ctx.client_locator.is_empty() {
                    tmux::current_session()
                } else {
                    tmux::current_session_for_tty(ctx.client_locator)
                };
                let mut sessions = tmux::list_sessions();
                for session in &mut sessions {
                    session.is_current = current.as_deref() == Some(session.name.as_str());
                }
                let agents = ctx.probe_agents.then(|| {
                    let mut agents =
                        agent::detect_agents(&tmux::agent_panes(), &agent::ps_snapshot());
                    for detected in &mut agents {
                        if let Some(buffer) = tmux::capture_pane(&detected.pane_id) {
                            detected.status = agent::classify_status(detected.kind, &buffer);
                        }
                    }
                    agents
                });
                Ok(LaneSnapshot { sessions, agents })
            }
            Some(host) => {
                // A failed ssh+tmux listing stays typed; don't probe agents
                // after either failure because a dead host would pay the 5s
                // timeout twice.
                let sessions = remote_tmux::list_sessions(host).map_err(|error| match error {
                    remote_tmux::ListSessionsError::Unreachable(detail) => {
                        CatalogError::Unreachable(detail)
                    }
                    remote_tmux::ListSessionsError::Backend(detail) => {
                        CatalogError::Backend(detail)
                    }
                })?;
                let agents = ctx
                    .probe_agents
                    .then(|| remote_tmux::agent_probe(host))
                    .flatten();
                Ok(LaneSnapshot { sessions, agents })
            }
        }
    }

    fn snapshot_mode(&self, lane: &LaneId) -> SnapshotMode {
        if Self::host_of(lane).is_none() {
            SnapshotMode::Foreground
        } else {
            SnapshotMode::Background
        }
    }
}

impl SessionControlProvider for TmuxSystem {
    fn control(&self, lane: &LaneId, ctx: &ControlCtx) -> Box<dyn SessionControl + Send> {
        match TmuxSystem::host_of(lane) {
            None => Box::new(LocalControl::new(ctx.local_client.to_string())),
            Some(host) => {
                let marker_id = ctx.connection_generations.get(lane).copied().unwrap_or(0);
                Box::new(RemoteControl::new(host.to_string(), marker_id))
            }
        }
    }
}

impl LaneActionProvider for TmuxSystem {
    fn invoke(
        &self,
        lane: &LaneId,
        action: &LaneActionId,
        anchor: LaneActionAnchor,
    ) -> Vec<LaneShellIntent> {
        let host = TmuxSystem::host_of(lane);
        match action.as_str() {
            // The generic menu button this system owns.
            cmd::MENU => vec![LaneShellIntent::OpenContextMenu { anchor }],
            cmd::NEW_SESSION if host.is_none() => vec![LaneShellIntent::OpenNewSession],
            // Everything else on a remote divider is ssh-registered; route it
            // back to ssh, which owns those commands' semantics.
            _ => match host {
                Some(_) => crate::ssh::divider::invoke(action),
                None => vec![],
            },
        }
    }
}
