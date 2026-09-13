use super::*;

fn ip(last: u8) -> IpAddr {
    IpAddr::from([192, 168, 1, last])
}

fn worker() -> BuddyWorker {
    BuddyWorker::new()
}

/// Hand a connection to the worker, and raise the prompt it queued if the
/// screen is free — what the caller does on every tick.
fn connect(
    w: &mut BuddyWorker,
    peer: IpAddr,
    screen_busy: bool,
) -> (Option<IpAddr>, Receiver<bool>) {
    let (reply, verdict) = mpsc::channel();
    w.connected(peer, reply);
    (w.next_question(screen_busy), verdict)
}

#[test]
fn an_unknown_device_is_asked_about() {
    let mut w = worker();
    let (asked, verdict) = connect(&mut w, ip(10), false);
    assert_eq!(asked, Some(ip(10)));
    // Nothing is decided until the user answers.
    assert!(verdict.try_recv().is_err());

    w.answer(true);
    assert_eq!(verdict.try_recv(), Ok(true));
}

#[test]
fn an_already_approved_device_is_let_in_without_a_prompt() {
    // The client reconnects on every network blip; a prompt each time would be
    // unusable.
    let mut w = worker();
    let (_, first) = connect(&mut w, ip(10), false);
    w.answer(true);
    assert_eq!(first.try_recv(), Ok(true));

    let (asked, second) = connect(&mut w, ip(10), false);
    assert_eq!(asked, None);
    assert_eq!(second.try_recv(), Ok(true));
}

#[test]
fn a_just_denied_device_is_refused_without_a_prompt() {
    // A client in its reconnect backoff must not be able to put the prompt
    // back up over and over.
    let mut w = worker();
    let (_, first) = connect(&mut w, ip(10), false);
    w.answer(false);
    assert_eq!(first.try_recv(), Ok(false));

    let (asked, second) = connect(&mut w, ip(10), false);
    assert_eq!(asked, None);
    assert_eq!(second.try_recv(), Ok(false));
}

#[test]
fn a_denial_lapses_so_a_mistaken_n_is_not_a_lockout() {
    let denied = HashMap::from([(ip(10), Instant::now())]);
    let now = Instant::now();
    assert_eq!(decide(ip(10), &[], &denied, now), Verdict::Deny);
    assert_eq!(
        decide(
            ip(10),
            &[],
            &denied,
            now + DENY_FOR + Duration::from_secs(1)
        ),
        Verdict::Ask
    );
    // Approval is not on a timer; only denial is.
    assert_eq!(decide(ip(10), &[ip(10)], &denied, now), Verdict::Allow);
}

#[test]
fn approving_clears_an_earlier_denial() {
    let mut w = worker();
    connect(&mut w, ip(10), false);
    w.answer(false);

    // Once the denial lapses the device asks again, and a yes must stick.
    w.denied.clear();
    let (asked, verdict) = connect(&mut w, ip(10), false);
    assert_eq!(asked, Some(ip(10)));
    w.answer(true);
    assert_eq!(verdict.try_recv(), Ok(true));
    assert!(!w.denied.contains_key(&ip(10)));
}

#[test]
fn nothing_is_asked_while_another_overlay_owns_the_screen() {
    // The modal slot holds one thing and `open` replaces it, so prompting over
    // a rename would destroy what the user was typing.
    let mut w = worker();
    let (asked, verdict) = connect(&mut w, ip(10), true);
    assert_eq!(asked, None);
    assert!(verdict.try_recv().is_err(), "it was answered, not queued");

    // It is still waiting, and goes up as soon as the screen is free.
    assert!(w.is_asking());
    assert_eq!(w.next_question(true), None);
    assert_eq!(w.next_question(false), Some(ip(10)));
}

#[test]
fn a_question_queued_behind_an_overlay_still_freezes_every_connection() {
    // The freeze is what makes leaving one queued unacceptable: an approved
    // iPad stops working until the prompt is answered.
    let mut w = worker();
    connect(&mut w, ip(10), true);
    assert!(w.is_asking(), "queued but outstanding");
}

#[test]
fn a_second_device_waits_its_turn_rather_than_replacing_the_prompt() {
    let mut w = worker();
    assert_eq!(connect(&mut w, ip(10), false).0, Some(ip(10)));
    let (asked, second) = connect(&mut w, ip(11), false);
    assert_eq!(asked, None);

    w.answer(true);
    assert!(second.try_recv().is_err(), "answered the wrong device");
    assert_eq!(w.next_question(false), Some(ip(11)));
    w.answer(true);
    assert_eq!(second.try_recv(), Ok(true));
}

#[test]
fn a_device_that_leaves_takes_its_question_with_it() {
    let mut w = worker();
    connect(&mut w, ip(10), false);
    assert!(w.disconnected(ip(10)), "the prompt was about this device");
    assert!(!w.is_asking());

    // One leaving from the queue does not take the prompt down.
    connect(&mut w, ip(10), false);
    connect(&mut w, ip(11), false);
    assert!(!w.disconnected(ip(11)));
    assert_eq!(w.asking_peer(), Some(ip(10)));
}

#[test]
fn the_freeze_is_on_exactly_while_a_question_is_outstanding() {
    // An approved device can type, and `y` answers the prompt — so while any
    // question is up, nothing may act.
    let mut w = worker();
    assert!(!w.is_asking());
    connect(&mut w, ip(10), false);
    assert!(w.is_asking());
    connect(&mut w, ip(11), false);
    w.answer(true);
    assert!(w.is_asking(), "still one queued");
    w.next_question(false);
    w.answer(true);
    assert!(!w.is_asking());
}

#[test]
fn an_approval_is_reported_once_so_the_caller_can_persist_it() {
    let mut w = worker();
    connect(&mut w, ip(10), false);
    assert_eq!(w.answer(true), Some(ip(10)), "new device, save it");

    // Reconnecting is allowed from memory and needs no second write.
    connect(&mut w, ip(10), false);
    assert_eq!(w.answer(true), None);

    // A denial is remembered only in this process, so nothing to persist.
    connect(&mut w, ip(11), false);
    assert_eq!(w.answer(false), None);
}

#[test]
fn a_device_approved_in_an_earlier_run_is_not_asked_about_again() {
    // The whole point: approving an iPad once, not once per deck launch.
    let mut w = worker();
    w.restore_approved(&["192.168.1.10".to_string(), "not-an-address".to_string()]);
    let (asked, verdict) = connect(&mut w, ip(10), false);
    assert_eq!(asked, None, "should not prompt");
    assert_eq!(verdict.try_recv(), Ok(true));
}

#[test]
fn dropping_a_saved_address_revokes_it_on_reload() {
    // Config is the source of truth, so editing it is how a device is removed.
    let mut w = worker();
    w.restore_approved(&["192.168.1.10".to_string()]);
    w.restore_approved(&[]);
    let (asked, _) = connect(&mut w, ip(10), false);
    assert_eq!(asked, Some(ip(10)), "asked about again");
}

#[test]
fn answering_nothing_is_harmless() {
    let mut w = worker();
    w.answer(true);
    assert!(!w.is_asking());
}
