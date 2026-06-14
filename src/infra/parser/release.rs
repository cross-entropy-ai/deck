//! Parser for the GitHub releases API JSON. The HTTP fetch lives in
//! `infra::update`; this turns the response body into `(version, url)`.

use serde::Deserialize;

/// Parse a GitHub release JSON body into `(version, html_url)`. The leading
/// `v` on the tag (`v0.2.0`) is stripped so the result compares cleanly with
/// the crate version.
pub fn parse_release_json(body: &str) -> Result<(String, String), String> {
    #[derive(Deserialize)]
    struct Release {
        tag_name: String,
        html_url: String,
    }
    let r: Release = serde_json::from_str(body).map_err(|e| format!("parse: {}", e))?;
    let version = r.tag_name.trim_start_matches('v').to_string();
    Ok((version, r.html_url))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_release_strips_v_prefix() {
        let body = r#"{"tag_name":"v0.2.0","html_url":"https://example.com/tag"}"#;
        let (ver, url) = parse_release_json(body).unwrap();
        assert_eq!(ver, "0.2.0");
        assert_eq!(url, "https://example.com/tag");
    }

    #[test]
    fn parse_release_without_v_prefix_ok() {
        let body = r#"{"tag_name":"0.2.0","html_url":"https://example.com/tag"}"#;
        let (ver, _) = parse_release_json(body).unwrap();
        assert_eq!(ver, "0.2.0");
    }

    #[test]
    fn parse_release_missing_field_errors() {
        let body = r#"{"tag_name":"v0.2.0"}"#;
        assert!(parse_release_json(body).is_err());
    }

    #[test]
    fn parse_release_invalid_json_errors() {
        assert!(parse_release_json("not json").is_err());
    }
}
