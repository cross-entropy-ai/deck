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

// --- PfAddForm::plan_submit ---

use crate::forwards::{ForwardEndpointKind, PfAddForm, PfSubmit};
use crate::new_session::make_textarea;

/// A form filled in well enough to validate: forward 8080 to example.com:80.
fn filled_form() -> PfAddForm {
    let mut form = PfAddForm::default_for(ForwardMode::Local, ForwardEndpointKind::Explicit, "box");
    form.listen_port = make_textarea("8080");
    form.target_host = make_textarea("example.com");
    form.target_port = make_textarea("80");
    form
}

fn forward_on(listen_port: u16) -> ForwardSpec {
    ForwardSpec {
        mode: ForwardMode::Local,
        bind_addr: Some("0.0.0.0".into()),
        listen_port,
        target_host: Some("example.com".into()),
        target_port: Some(80),
    }
}

#[test]
fn a_valid_form_on_a_free_port_submits() {
    let PfSubmit::Add(spec) = filled_form().plan_submit(true, &[]) else {
        panic!("a valid form on a free port must submit");
    };
    assert_eq!(spec.listen_port, 8080);
    assert_eq!(spec.target_host.as_deref(), Some("example.com"));
}

/// Forwards ride the ControlMaster socket, so without reuse there is nothing
/// to attach one to. Say where to turn it on rather than failing at ssh.
#[test]
fn reuse_being_off_is_refused_with_the_setting_that_fixes_it() {
    let PfSubmit::Refuse { status, focus } = filled_form().plan_submit(false, &[]) else {
        panic!("reuse off must refuse");
    };
    assert!(status.contains("Settings"), "{status:?}");
    assert_eq!(focus, None, "no field is at fault here");
}

/// Reuse is checked before the fields are: an empty form with reuse off gets
/// the setting hint, not a validation complaint it cannot act on yet.
#[test]
fn reuse_is_checked_before_the_fields_are() {
    let empty = PfAddForm::default_for(ForwardMode::Local, ForwardEndpointKind::Explicit, "box");
    let PfSubmit::Refuse { status, .. } = empty.plan_submit(false, &[]) else {
        panic!("reuse off must refuse");
    };
    assert!(status.contains("Settings"), "{status:?}");
}

/// A validation failure moves the cursor to the field that resolves it, so
/// the next keystroke edits the right box.
#[test]
fn an_invalid_field_refuses_and_points_at_itself() {
    let mut form = filled_form();
    form.listen_port = make_textarea("99999");
    let PfSubmit::Refuse { status, focus } = form.plan_submit(true, &[]) else {
        panic!("an out-of-range port must refuse");
    };
    assert_eq!(status, PfFormError::ListenPortRange.message());
    assert_eq!(focus, Some(PfField::ListenPort));
}

/// Caught here rather than at ssh, which answers a duplicate listener with a
/// cryptic bind error or, for some modes, silently nothing.
#[test]
fn a_port_already_forwarded_is_refused_before_ssh_sees_it() {
    let PfSubmit::Refuse { status, focus } = filled_form().plan_submit(true, &[forward_on(8080)])
    else {
        panic!("a duplicate listener must refuse");
    };
    assert!(
        status.contains("8080"),
        "the refusal must name the port: {status:?}"
    );
    assert_eq!(focus, None);
}

/// Only the listen identity collides. A different port on the same lane is a
/// perfectly good second forward.
#[test]
fn a_different_port_on_the_same_lane_still_submits() {
    assert!(matches!(
        filled_form().plan_submit(true, &[forward_on(9090)]),
        PfSubmit::Add(_)
    ));
}

/// The second Enter while the first is still out changes nothing — including
/// the status line, which still says the first one is applying.
#[test]
fn a_submit_already_in_flight_ignores_the_next_one() {
    let mut form = filled_form();
    form.submitting = true;
    assert_eq!(form.plan_submit(true, &[]), PfSubmit::Ignore);
}
