//! SSH's contribution to the Settings page. Implements the shared
//! [`SettingsProvider`](crate::settings_framework::SettingsProvider) contract
//! and is registered independently in `app::settings::SETTINGS_PROVIDERS` —
//! the ssh backend owns these rows, not the tmux system.

use crate::effects::Effect;
use crate::settings_framework::{SettingDef, SettingsCtx};

/// SSH settings rows: add a remote host, and open a host's port forwards.
pub fn rows(ctx: &SettingsCtx) -> Vec<SettingDef> {
    let total_forwards: usize = ctx.remotes.iter().map(|r| r.forwards.len()).sum();
    // Forwards are per-host; this aggregate row opens the editor for the first
    // host that has forwards (else the first host). The fix-up the badge button
    // on each `@host` divider offers is still the per-host route.
    let target = ctx
        .remotes
        .iter()
        .find(|r| !r.forwards.is_empty())
        .or_else(|| ctx.remotes.first())
        .map(|r| r.host.clone());

    vec![
        SettingDef {
            label: "Remotes",
            value: format!("{} hosts", ctx.remotes.len()),
            help: "Left/right adds a remote SSH host".to_string(),
            effect: Box::new(|_| vec![Effect::OpenAddRemotePicker]),
        },
        SettingDef {
            label: "Port forwards",
            value: match total_forwards {
                0 => "none".to_string(),
                n => format!("{n} forwards"),
            },
            help: "Left/right opens a configured host's port forwards".to_string(),
            effect: Box::new(move |_| {
                target
                    .clone()
                    .map(|h| vec![Effect::OpenForwardOverlay(h)])
                    .unwrap_or_default()
            }),
        },
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::RemoteConfig;
    use crate::forwards::{ForwardMode, ForwardSpec};

    fn spec(port: u16) -> ForwardSpec {
        ForwardSpec {
            mode: ForwardMode::Local,
            bind_addr: None,
            listen_port: port,
            target_host: Some("localhost".into()),
            target_port: Some(80),
        }
    }

    #[test]
    fn port_forwards_row_aggregates_across_hosts_and_targets_a_host() {
        let remotes = vec![
            RemoteConfig {
                host: "a".into(),
                forwards: vec![],
            },
            RemoteConfig {
                host: "b".into(),
                forwards: vec![spec(8080), spec(9090)],
            },
        ];
        let rows = rows(&SettingsCtx { remotes: &remotes });
        let pf = rows.iter().find(|r| r.label == "Port forwards").unwrap();
        assert_eq!(pf.value, "2 forwards");
        // Opens the first host that actually has forwards ("b"), not "a".
        assert!(matches!(
            (pf.effect)(1).as_slice(),
            [Effect::OpenForwardOverlay(h)] if h == "b"
        ));
    }

    #[test]
    fn port_forwards_row_is_noop_without_hosts() {
        let rows = rows(&SettingsCtx { remotes: &[] });
        let pf = rows.iter().find(|r| r.label == "Port forwards").unwrap();
        assert_eq!(pf.value, "none");
        assert!((pf.effect)(1).is_empty());
    }
}
