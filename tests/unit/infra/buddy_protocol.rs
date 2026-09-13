use super::*;

fn key_steps(raw: &str) -> Vec<Step> {
    match parse(raw.as_bytes()) {
        Some(BuddyMsg::Key { steps }) => steps,
        other => panic!("expected a key message, got {other:?}"),
    }
}

#[test]
fn a_chord_carries_its_modifiers() {
    let steps = key_steps(r#"{"type":"key","steps":[{"key":"c","modifiers":["command"]}]}"#);
    assert_eq!(steps.len(), 1);
    assert_eq!(steps[0].key, "c");
    assert_eq!(steps[0].modifiers(), ["command"]);
}

#[test]
fn an_unmodified_step_reads_as_no_modifiers_however_it_was_written() {
    // The client omits the key; the Python also tolerated an explicit null.
    for raw in [
        r#"{"type":"key","steps":[{"key":"a"}]}"#,
        r#"{"type":"key","steps":[{"key":"a","modifiers":null}]}"#,
    ] {
        assert!(key_steps(raw)[0].modifiers().is_empty(), "{raw}");
    }
}

#[test]
fn text_and_mouse_messages_round_trip() {
    assert_eq!(
        parse(br#"{"type":"text","text":"hi"}"#),
        Some(BuddyMsg::Text {
            text: "hi".to_string()
        })
    );
    let Some(BuddyMsg::Mouse(mouse)) = parse(br#"{"type":"mouse","action":"move","dx":3,"dy":-4}"#)
    else {
        panic!("expected a mouse message");
    };
    assert_eq!(mouse.action, MouseAction::Move);
    assert_eq!((mouse.dx, mouse.dy), (3, -4));
    // Absent fields take the client's own defaults rather than failing.
    assert_eq!(mouse.button, MouseButton::Left);
    assert_eq!(mouse.count, 1);
}

#[test]
fn a_click_keeps_its_button_and_count() {
    let Some(BuddyMsg::Mouse(mouse)) =
        parse(br#"{"type":"mouse","action":"click","button":"right","count":2}"#)
    else {
        panic!("expected a mouse message");
    };
    assert_eq!(mouse.button, MouseButton::Right);
    assert_eq!(mouse.count, 2);
}

#[test]
fn a_ping_parses_from_its_tag_alone() {
    assert_eq!(parse(br#"{"type":"ping"}"#), Some(BuddyMsg::Ping));
    // The reply is the one the client matches on.
    assert_eq!(PONG, r#"{"type":"pong"}"#);
}

#[test]
fn anything_unrecognised_is_dropped_rather_than_erroring() {
    // A newer client's button should do nothing, not kill the connection.
    for raw in [
        "not json at all",
        r#"{"type":"clipboard","text":"x"}"#,
        r#"{"type":"mouse","action":"teleport"}"#,
        "",
    ] {
        assert_eq!(parse(raw.as_bytes()), None, "{raw}");
    }
}

#[test]
fn key_names_resolve_case_insensitively_through_both_tables() {
    assert_eq!(key_code("escape"), Some(0x35));
    assert_eq!(key_code("ESC"), Some(0x35));
    assert_eq!(key_code("Return"), key_code("enter"));
    assert_eq!(key_code("delete"), key_code("backspace"));
    assert_eq!(key_code("f12"), Some(0x6F));
    // A bare character falls through to the ANSI layout, either case.
    assert_eq!(key_code("c"), Some(0x08));
    assert_eq!(key_code("C"), Some(0x08));
    assert_eq!(key_code("-"), Some(0x1B));
    // No keycode for these; the caller uses the Unicode path instead.
    assert_eq!(key_code("中"), None);
    assert_eq!(key_code("nosuchkey"), None);
    assert_eq!(key_code(""), None);
}

#[test]
fn every_f_key_the_client_can_send_has_a_distinct_code() {
    let codes: Vec<u16> = (1..=20)
        .map(|n| key_code(&format!("f{n}")).unwrap_or_else(|| panic!("f{n} is unmapped")))
        .collect();
    let mut sorted = codes.clone();
    sorted.sort_unstable();
    sorted.dedup();
    assert_eq!(
        sorted.len(),
        codes.len(),
        "two F-keys share a code: {codes:?}"
    );
}

#[test]
fn modifier_names_accumulate_into_one_mask() {
    assert_eq!(modifier_flags(&[]), 0);
    let all = ["command", "ctrl", "shift", "alt"].map(String::from);
    assert_eq!(
        modifier_flags(&all),
        FLAG_COMMAND | FLAG_CONTROL | FLAG_SHIFT | FLAG_ALTERNATE
    );
    // Aliases agree, and an unknown name contributes nothing.
    let aliases = ["cmd", "control", "opt", "banana"].map(String::from);
    assert_eq!(
        modifier_flags(&aliases),
        FLAG_COMMAND | FLAG_CONTROL | FLAG_ALTERNATE
    );
}

#[test]
fn chunking_respects_the_utf16_limit_without_splitting_a_character() {
    assert_eq!(utf16_chunks("", 4), Vec::<&str>::new());
    assert_eq!(utf16_chunks("abcd", 4), ["abcd"]);
    assert_eq!(utf16_chunks("abcde", 4), ["abcd", "e"]);
    // An emoji is two UTF-16 units, so only two fit in a limit of four — and
    // the boundary lands between them, never inside one.
    assert_eq!(utf16_chunks("😀😀😀", 4), ["😀😀", "😀"]);
    for chunk in utf16_chunks("中文字符测试", 3) {
        assert!(chunk.chars().count() <= 3, "{chunk:?} is over the limit");
    }
    assert_eq!(utf16_chunks("中文字符测试", 3).concat(), "中文字符测试");
}

#[test]
fn a_character_wider_than_the_limit_still_goes_out_whole() {
    // Never emit an empty chunk or split the character to obey the limit;
    // truncating one emoji into two lone surrogates would corrupt it.
    assert_eq!(utf16_chunks("😀a", 1), ["😀", "a"]);
}

#[test]
fn a_state_request_parses_from_its_tag_alone() {
    assert_eq!(parse(br#"{"type":"state"}"#), Some(BuddyMsg::State));
}

#[test]
fn each_selection_reads_from_the_key_that_names_it() {
    // One `select` type with the target as its single key, so the three
    // selections share a shape instead of needing three tags.
    assert_eq!(
        parse(br#"{"type":"select","tab":"agents"}"#),
        Some(BuddyMsg::Select(Select::Tab(Tab::Agents)))
    );
    assert_eq!(
        parse(br#"{"type":"select","session":{"lane":"tmux\u001flocal","name":"deck"}}"#),
        Some(BuddyMsg::Select(Select::Session(SessionRef {
            lane: "tmux\u{1f}local".to_string(),
            name: "deck".to_string(),
        })))
    );
    assert_eq!(
        parse(br#"{"type":"select","agent":{"lane":"tmux\u001fbox","pane":"%7"}}"#),
        Some(BuddyMsg::Select(Select::Agent(AgentRef {
            lane: "tmux\u{1f}box".to_string(),
            pane: "%7".to_string(),
        })))
    );
}

#[test]
fn the_session_tab_answers_to_both_of_its_names() {
    // `projects` is deck's own word for it; `sessions` is what the tab shows,
    // and a client that says either means the same list.
    for raw in [
        br#"{"type":"select","tab":"projects"}"#.as_slice(),
        br#"{"type":"select","tab":"sessions"}"#.as_slice(),
    ] {
        assert_eq!(
            parse(raw),
            Some(BuddyMsg::Select(Select::Tab(Tab::Projects))),
            "{}",
            String::from_utf8_lossy(raw)
        );
    }
}

#[test]
fn a_selection_naming_nothing_known_is_dropped_not_rejected() {
    // Same rule as every other unknown shape: a client from a newer app
    // degrades to "that button does nothing", never to a dropped connection.
    for raw in [
        br#"{"type":"select"}"#.as_slice(),
        br#"{"type":"select","window":{"lane":"x"}}"#.as_slice(),
        br#"{"type":"select","tab":"settings"}"#.as_slice(),
        br#"{"type":"select","session":{"lane":"x"}}"#.as_slice(),
    ] {
        assert_eq!(parse(raw), None, "{}", String::from_utf8_lossy(raw));
    }
}

#[test]
fn a_state_reply_carries_its_type_and_three_lists() {
    let reply = State {
        tab: Tab::Agents,
        hosts: vec![HostInfo {
            lane: "tmux\u{1f}local".to_string(),
            title: "local".to_string(),
            parent: None,
            status: HostStatus::Ok,
        }],
        sessions: vec![SessionInfo {
            lane: "tmux\u{1f}local".to_string(),
            name: "deck".to_string(),
            dir: "~/claude/deck".to_string(),
            selected: true,
        }],
        agents: vec![AgentInfo {
            lane: "tmux\u{1f}local".to_string(),
            kind: AgentKindName::Claude,
            session: "deck".to_string(),
            window: "main".to_string(),
            pane: "%3".to_string(),
            status: AgentStatusName::Waiting,
            selected: false,
        }],
    };
    // Spelled out rather than compared field by field: this string is the
    // contract the client decodes, so a rename has to fail here.
    assert_eq!(
        reply.encode(),
        concat!(
            r#"{"type":"state","tab":"agents","#,
            r#""hosts":[{"lane":"tmux\u001flocal","title":"local","parent":null,"status":"ok"}],"#,
            r#""sessions":[{"lane":"tmux\u001flocal","name":"deck","dir":"~/claude/deck","selected":true}],"#,
            r#""agents":[{"lane":"tmux\u001flocal","kind":"claude","session":"deck","window":"main","#,
            r#""pane":"%3","status":"waiting","selected":false}]}"#,
        )
    );
}

#[test]
fn the_placeholder_reasons_keep_their_wire_names() {
    // A lane with nothing to list says why here, since it contributes no
    // session row to say it with.
    let named = |status| {
        State {
            tab: Tab::Projects,
            hosts: vec![HostInfo {
                lane: "tmux\u{1f}box".to_string(),
                title: "box".to_string(),
                parent: Some("tmux\u{1f}local".to_string()),
                status,
            }],
            sessions: Vec::new(),
            agents: Vec::new(),
        }
        .encode()
    };
    assert!(named(HostStatus::Connecting).contains(r#""status":"connecting""#));
    assert!(named(HostStatus::Unreachable).contains(r#""status":"unreachable""#));
    assert!(named(HostStatus::NoSessions).contains(r#""status":"no_sessions""#));
    assert!(named(HostStatus::Ok).contains(r#""parent":"tmux\u001flocal""#));
}
