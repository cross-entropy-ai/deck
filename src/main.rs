mod app;
mod infra;
mod model;
mod session;
mod system;
mod ui;

pub(crate) use app::action;
pub(crate) use infra::guards::{instance_guard, preflight_guard, terminal_guard};
pub(crate) use infra::ssh::model::{add_remote, forwards};
pub(crate) use infra::tmux::{local as tmux, remote as remote_tmux};
pub(crate) use infra::{
    agent, focus, pty, refresh, self_update, shutdown, ssh, summary, update, worker,
};
pub(crate) use model::{
    config, effects, exclude, geometry, keybindings, lane, menu, new_session, overlay, picker,
    state, summary as summary_card,
};
pub(crate) use ui::{bridge, theme};

mod cli;
use cli::{parse_args, ParsedCommand};

use std::io;

use instance_guard::InstanceGuard;
use preflight_guard::PreflightGuard;
use terminal_guard::TerminalGuard;

fn main() -> io::Result<()> {
    let args = match parse_args(std::env::args().skip(1)) {
        Ok(Some(ParsedCommand::Run(args))) => args,
        Ok(Some(ParsedCommand::RemoteAdd(host))) => {
            std::process::exit(cli::run_remote_add(&host));
        }
        Ok(Some(ParsedCommand::RemoteRemove(host))) => {
            std::process::exit(cli::run_remote_remove(&host));
        }
        Ok(Some(ParsedCommand::RemoteList)) => {
            cli::run_remote_list();
            return Ok(());
        }
        Ok(Some(ParsedCommand::UpgradeSelf(version))) => {
            match self_update::run_self_upgrade(&version) {
                Ok(()) => return Ok(()),
                Err(e) => {
                    eprintln!("deck: upgrade failed: {e}");
                    std::process::exit(1);
                }
            }
        }
        Ok(None) => return Ok(()),
        Err(code) => std::process::exit(code),
    };

    // Install the SIGTERM handler before we acquire the lock, so a
    // concurrent `deck --force` that targets us is handled as soon as
    // the flag lands rather than hitting the default terminate action.
    if let Err(err) = shutdown::install_sigterm_handler() {
        eprintln!("deck: failed to install SIGTERM handler: {err}");
    }

    let _instance_guard = InstanceGuard::acquire_for_current_process_or_exit(args.force)?;

    if let Err(err) = PreflightGuard::run(args.attach_override.as_deref(), args.create_new) {
        eprintln!("deck: {err}");
        std::process::exit(1);
    }

    ratatui::run(|terminal| {
        let _terminal_guard = TerminalGuard::enter()?;
        let size = terminal.size()?;
        let mut app = app::App::new(size.width, size.height, args.attach_override.clone())?;
        app.run(terminal)
    })?;

    Ok(())
}
