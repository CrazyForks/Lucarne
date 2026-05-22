use reqwest::header::{ACCEPT, USER_AGENT};
use serde::Deserialize;

use super::UpdateError;

const GITHUB_API_ACCEPT: &str = "application/vnd.github+json";
const MAX_RESPONSE_BYTES: usize = 128 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GithubRelease {
    pub tag_name: String,
    pub name: String,
    pub html_url: String,
    pub body: String,
    pub published_at: Option<String>,
    pub draft: bool,
    pub prerelease: bool,
}

#[derive(Debug, Deserialize)]
struct GithubReleaseJson {
    tag_name: String,
    #[serde(default)]
    name: String,
    html_url: String,
    #[serde(default)]
    body: String,
    published_at: Option<String>,
    #[serde(default)]
    draft: bool,
    #[serde(default)]
    prerelease: bool,
}

pub fn parse_release_json(raw: &str) -> Result<GithubRelease, serde_json::Error> {
    let json: GithubReleaseJson = serde_json::from_str(raw)?;
    Ok(GithubRelease {
        tag_name: json.tag_name,
        name: json.name,
        html_url: json.html_url,
        body: json.body,
        published_at: json.published_at,
        draft: json.draft,
        prerelease: json.prerelease,
    })
}

pub async fn fetch_latest_release(
    client: &reqwest::Client,
    repository: &str,
    user_agent: &str,
) -> Result<Option<GithubRelease>, UpdateError> {
    if !is_valid_repository(repository) {
        return Err(UpdateError::InvalidRepository(repository.to_string()));
    }

    let url = format!("https://api.github.com/repos/{repository}/releases/latest");
    let mut response = client
        .get(url)
        .header(USER_AGENT, user_agent)
        .header(ACCEPT, GITHUB_API_ACCEPT)
        .send()
        .await?;

    let status = response.status();
    if !status.is_success() {
        return Err(UpdateError::HttpStatus(status));
    }

    if let Some(length) = response.content_length() {
        if length > MAX_RESPONSE_BYTES as u64 {
            return Err(UpdateError::ResponseTooLarge {
                limit: MAX_RESPONSE_BYTES,
                actual: length.min(usize::MAX as u64) as usize,
            });
        }
    }

    let mut body = Vec::new();
    while let Some(chunk) = response.chunk().await? {
        let actual = body.len().saturating_add(chunk.len());
        if actual > MAX_RESPONSE_BYTES {
            return Err(UpdateError::ResponseTooLarge {
                limit: MAX_RESPONSE_BYTES,
                actual,
            });
        }
        body.extend_from_slice(&chunk);
    }

    let body = String::from_utf8_lossy(&body);
    let release = parse_release_json(&body)?;
    if release.draft || release.prerelease {
        return Ok(None);
    }
    Ok(Some(release))
}

fn is_valid_repository(repository: &str) -> bool {
    let mut parts = repository.split('/');
    let (Some(owner), Some(repo), None) = (parts.next(), parts.next(), parts.next()) else {
        return false;
    };
    is_valid_repository_part(owner) && is_valid_repository_part(repo)
}

fn is_valid_repository_part(part: &str) -> bool {
    !part.is_empty()
        && part.bytes().all(
            |byte| matches!(byte, b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'.' | b'_' | b'-'),
        )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_normal_release_json() {
        let release = parse_release_json(
            r#"{
                "tag_name": "v0.2.0",
                "name": "Lucarne 0.2.0",
                "html_url": "https://github.com/tuchg/Lucarne/releases/tag/v0.2.0",
                "body": "Changes",
                "published_at": "2026-05-21T00:00:00Z",
                "draft": false,
                "prerelease": false
            }"#,
        )
        .unwrap();

        assert_eq!(release.tag_name, "v0.2.0");
        assert_eq!(release.name, "Lucarne 0.2.0");
        assert_eq!(
            release.html_url,
            "https://github.com/tuchg/Lucarne/releases/tag/v0.2.0"
        );
        assert_eq!(release.body, "Changes");
        assert_eq!(
            release.published_at.as_deref(),
            Some("2026-05-21T00:00:00Z")
        );
        assert!(!release.draft);
        assert!(!release.prerelease);
    }

    #[test]
    fn parses_prerelease_and_draft_flags() {
        let prerelease = parse_release_json(
            r#"{
                "tag_name": "v0.3.0-beta.1",
                "name": "Beta",
                "html_url": "https://example.invalid/beta",
                "prerelease": true
            }"#,
        )
        .unwrap();
        assert!(prerelease.prerelease);
        assert!(!prerelease.draft);

        let draft = parse_release_json(
            r#"{
                "tag_name": "v0.3.0",
                "name": "Draft",
                "html_url": "https://example.invalid/draft",
                "draft": true
            }"#,
        )
        .unwrap();
        assert!(draft.draft);
        assert!(!draft.prerelease);
    }

    #[test]
    fn missing_optional_fields_default() {
        let release = parse_release_json(
            r#"{
                "tag_name": "v0.2.0",
                "html_url": "https://example.invalid/release"
            }"#,
        )
        .unwrap();

        assert_eq!(release.name, "");
        assert_eq!(release.body, "");
        assert_eq!(release.published_at, None);
        assert!(!release.draft);
        assert!(!release.prerelease);
    }

    #[tokio::test]
    async fn invalid_repository_is_rejected_before_request() {
        let client = reqwest::Client::new();
        for repository in [
            "tuchg",
            "tuchg/",
            "/Lucarne",
            "tuchg/Lucarne/extra",
            "bad repo/Lucarne",
        ] {
            let err = fetch_latest_release(&client, repository, "lucarned-test")
                .await
                .unwrap_err();
            assert!(
                matches!(err, UpdateError::InvalidRepository(invalid) if invalid == repository)
            );
        }
    }
}
