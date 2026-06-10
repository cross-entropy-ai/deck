use super::*;

fn cap(host: Option<&str>, id: &str, content: &str) -> PaneCapture {
    PaneCapture {
        host: host.map(str::to_string),
        id: id.to_string(),
        content: content.to_string(),
    }
}

#[test]
fn build_prompt_splices_session_blocks_at_placeholder() {
    let template = "intro\n{{SESSIONS}}\noutro";
    let out = build_prompt(
        template,
        &[
            cap(None, "deck:1.1", "local buffer"),
            cap(Some("h1"), "work:0.2", "remote buffer"),
        ],
    );
    assert!(out.starts_with("intro\n"));
    assert!(out.ends_with("\noutro"));
    assert!(out.contains("<session id=\"deck:1.1\">\nlocal buffer\n</session>"));
    assert!(out.contains("<session id=\"work:0.2\" host=\"h1\">\nremote buffer\n</session>"));
    // The placeholder itself is gone.
    assert!(!out.contains(PROMPT_PLACEHOLDER));
}

#[test]
fn build_prompt_appends_when_no_placeholder() {
    let out = build_prompt("just a prompt", &[cap(None, "a:0.0", "buf")]);
    assert!(out.starts_with("just a prompt\n\n"));
    assert!(out.contains("<session id=\"a:0.0\">\nbuf\n</session>"));
}

#[test]
fn build_prompt_escapes_attribute_specials() {
    let out = build_prompt(
        "{{SESSIONS}}",
        &[cap(Some("a&b<\"c"), "x\"y", "content & <stuff>")],
    );
    // Attributes are escaped...
    assert!(out.contains("host=\"a&amp;b&lt;&quot;c\""));
    assert!(out.contains("id=\"x&quot;y\""));
    // ...but the content is left raw for the model.
    assert!(out.contains("content & <stuff>"));
}

#[test]
fn default_prompt_carries_the_placeholder() {
    assert!(DEFAULT_SUMMARY_PROMPT.contains(PROMPT_PLACEHOLDER));
}

#[test]
fn cycle_language_wraps_both_ways() {
    // From default ("") forward → first real language.
    assert_eq!(cycle_language("", 1), SUMMARY_LANGUAGES[1]);
    // From default backward → last entry (wrap).
    assert_eq!(cycle_language("", -1), SUMMARY_LANGUAGES[SUMMARY_LANGUAGES.len() - 1]);
    // An unknown value is treated as index 0.
    assert_eq!(cycle_language("Klingon", 1), SUMMARY_LANGUAGES[1]);
}

#[test]
fn language_label_shows_default_for_empty() {
    assert_eq!(language_label(""), "Default");
    assert_eq!(language_label("中文"), "中文");
}
