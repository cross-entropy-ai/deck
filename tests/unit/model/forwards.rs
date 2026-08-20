use crate::forwards::{diff_forwards, ForwardMode, ForwardOp, ForwardSpec, PfField, PfFormError};

#[test]
fn form_errors_point_to_the_field_that_resolves_them() {
    assert_eq!(PfFormError::ListenPortRange.field(), PfField::ListenPort);
    assert_eq!(PfFormError::TargetPortRange.field(), PfField::TargetPort);
    assert_eq!(PfFormError::TargetHostRequired.field(), PfField::TargetHost);
}

#[test]
fn forward_spec_local_to_flag_no_bind() {
    let spec = ForwardSpec {
        mode: ForwardMode::Local,
        bind_addr: None,
        listen_port: 8080,
        target_host: Some("example.com".into()),
        target_port: Some(80),
    };
    assert_eq!(spec.to_ssh_flag(), "-L 8080:example.com:80");
}

#[test]
fn forward_spec_remote_to_flag() {
    let spec = ForwardSpec {
        mode: ForwardMode::Remote,
        bind_addr: Some("0.0.0.0".into()),
        listen_port: 9090,
        target_host: Some("localhost".into()),
        target_port: Some(5432),
    };
    assert_eq!(spec.to_ssh_flag(), "-R 0.0.0.0:9090:localhost:5432");
}

#[test]
fn forward_spec_dynamic_to_flag() {
    let spec = ForwardSpec {
        mode: ForwardMode::Dynamic,
        bind_addr: None,
        listen_port: 1080,
        target_host: None,
        target_port: None,
    };
    assert_eq!(spec.to_ssh_flag(), "-D 1080");
}

#[test]
fn forward_spec_dynamic_with_bind_to_flag() {
    let spec = ForwardSpec {
        mode: ForwardMode::Dynamic,
        bind_addr: Some("127.0.0.1".into()),
        listen_port: 1080,
        target_host: None,
        target_port: None,
    };
    assert_eq!(spec.to_ssh_flag(), "-D 127.0.0.1:1080");
}

fn fwd(port: u16) -> ForwardSpec {
    ForwardSpec {
        mode: ForwardMode::Local,
        bind_addr: None,
        listen_port: port,
        target_host: Some("localhost".into()),
        target_port: Some(80),
    }
}

#[test]
fn diff_forwards_unchanged_emits_nothing() {
    let v = vec![fwd(8080)];
    let ops = diff_forwards(&v, &v);
    assert!(ops.is_empty());
}

#[test]
fn diff_forwards_mixed() {
    let old = vec![fwd(8080), fwd(9090)];
    let new = vec![fwd(8080), fwd(7070)];
    let ops = diff_forwards(&old, &new);
    assert_eq!(ops.len(), 2);
    assert!(ops
        .iter()
        .any(|o| matches!(o, ForwardOp::Cancel(s) if s.listen_port == 9090)));
    assert!(ops
        .iter()
        .any(|o| matches!(o, ForwardOp::Add(s) if s.listen_port == 7070)));
}

#[test]
fn a_lane_that_is_its_own_endpoint_asks_for_a_port_and_nothing_else() {
    use crate::forwards::{PfAddForm, PfField};
    use crate::system::ForwardEndpointKind;

    let mut form =
        PfAddForm::default_for(ForwardMode::Local, ForwardEndpointKind::Lane, "devbox/dev");
    // `-R` puts the listener on the far side and `-D` picks a destination per
    // connection; neither one would ever reach this lane, so neither is
    // offered rather than offered and then rejected.
    assert_eq!(form.modes(), &[ForwardMode::Local]);
    assert!(!form.asks_target_host());
    // The field is seeded with the lane's own name so the flow sketch reads as
    // the user thinks of it, and is never edited.
    assert_eq!(form.field_text(PfField::TargetHost), "devbox/dev");

    form.listen_port = crate::new_session::make_textarea("9000");
    form.target_port = crate::new_session::make_textarea("8080");
    let spec = form.validate().expect("valid");
    assert_eq!(spec.mode, ForwardMode::Local);
    assert_eq!(spec.listen_port, 9000);
    assert_eq!(spec.target_port, Some(8080));
    // No address is stored: a container's changes when it restarts, so the
    // worker resolves one on every apply instead.
    assert_eq!(spec.target_host, None);

    // A host lane is the other way round — every mode, and an address it must
    // be given.
    let host = PfAddForm::default_for(ForwardMode::Local, ForwardEndpointKind::Explicit, "devbox");
    assert_eq!(host.modes().len(), 3);
    assert!(host.asks_target_host());
    assert_eq!(host.field_text(PfField::TargetHost), "127.0.0.1");
}

#[test]
fn a_lane_endpoint_form_cannot_open_in_a_mode_it_does_not_offer() {
    use crate::forwards::PfAddForm;
    use crate::system::ForwardEndpointKind;

    // Whatever the caller asks for, only `-L` exists here.
    let form = PfAddForm::default_for(
        ForwardMode::Dynamic,
        ForwardEndpointKind::Lane,
        "devbox/dev",
    );
    assert_eq!(form.mode, ForwardMode::Local);
}
