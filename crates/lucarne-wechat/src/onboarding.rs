use std::{future::Future, path::Path};

use wechat_ilink::{Credentials, LoginQrEvent, WechatIlinkClient};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WechatOnboardingResult {
    pub reused_existing_credentials: bool,
}

pub type WechatOnboardingError = Box<dyn std::error::Error + Send + Sync>;

pub async fn ensure_wechat_onboarding_credentials(
    credential_path: impl AsRef<Path>,
    reuse_existing: bool,
    http_client: reqwest::Client,
) -> Result<WechatOnboardingResult, WechatOnboardingError> {
    ensure_wechat_onboarding_credentials_with_login(
        credential_path,
        reuse_existing,
        || async move { login_wechat_qr(http_client).await },
    )
    .await
}

async fn ensure_wechat_onboarding_credentials_with_login<Login, Fut>(
    credential_path: impl AsRef<Path>,
    reuse_existing: bool,
    login: Login,
) -> Result<WechatOnboardingResult, WechatOnboardingError>
where
    Login: FnOnce() -> Fut,
    Fut: Future<Output = Result<Credentials, WechatOnboardingError>>,
{
    let credential_path = credential_path.as_ref();
    if reuse_existing && credential_path.is_file() {
        return Ok(WechatOnboardingResult {
            reused_existing_credentials: true,
        });
    }

    if let Some(parent) = credential_path.parent() {
        if !parent.as_os_str().is_empty() {
            tokio::fs::create_dir_all(parent).await?;
        }
    }

    let credentials = login().await?;
    let mut json = serde_json::to_string_pretty(&credentials)?;
    json.push('\n');
    tokio::fs::write(credential_path, json).await?;

    Ok(WechatOnboardingResult {
        reused_existing_credentials: false,
    })
}

async fn login_wechat_qr(
    http_client: reqwest::Client,
) -> Result<Credentials, WechatOnboardingError> {
    let client = WechatIlinkClient::builder()
        .http_client(http_client)
        .build();
    let mut login = client.login_qr();

    while let Some(event) = login.next().await {
        match event? {
            LoginQrEvent::QrCode { content } => show_onboarding_login_qr(&content)?,
            LoginQrEvent::StatusChanged { status } => {
                eprintln!("[lucarne-wechat] WeChat login status: {status}");
            }
            LoginQrEvent::NeedVerifyCode { prompt, responder } => {
                let _ = responder.cancel();
                return Err(prompt.into());
            }
            LoginQrEvent::Confirmed { credentials } => return Ok(credentials),
        }
    }

    Err("wechat QR login stream ended before confirmation".into())
}

fn show_onboarding_login_qr(content: &str) -> Result<(), qrcode::types::QrError> {
    let qr = render_onboarding_terminal_qr(content)?;
    eprintln!("\n[lucarne-wechat] WeChat login required.\n{qr}");
    Ok(())
}

fn render_onboarding_terminal_qr(content: &str) -> Result<String, qrcode::types::QrError> {
    crate::adapter::render_terminal_qr(content)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    };

    fn test_credentials() -> wechat_ilink::Credentials {
        wechat_ilink::Credentials {
            token: "token".to_string(),
            base_url: "https://example.com".to_string(),
            account_id: "account".to_string(),
            user_id: "user".to_string(),
            saved_at: Some("2026-05-17T00:00:00Z".to_string()),
        }
    }

    #[tokio::test]
    async fn reuses_existing_credentials_when_allowed() {
        let dir = tempfile::tempdir().expect("tempdir");
        let credential_path = dir.path().join("wechat-credentials.json");
        std::fs::write(&credential_path, "{}").expect("write credentials");
        let calls = Arc::new(AtomicUsize::new(0));
        let calls_for_login = Arc::clone(&calls);

        let result =
            ensure_wechat_onboarding_credentials_with_login(&credential_path, true, move || {
                calls_for_login.fetch_add(1, Ordering::SeqCst);
                async { Ok(test_credentials()) }
            })
            .await
            .expect("onboarding succeeds");

        assert!(result.reused_existing_credentials);
        assert_eq!(calls.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn runs_login_when_credentials_missing() {
        let dir = tempfile::tempdir().expect("tempdir");
        let credential_path = dir.path().join("wechat-credentials.json");

        let result =
            ensure_wechat_onboarding_credentials_with_login(&credential_path, true, || async {
                Ok(test_credentials())
            })
            .await
            .expect("onboarding succeeds");

        assert!(!result.reused_existing_credentials);
        let content = std::fs::read_to_string(&credential_path).expect("credentials written");
        assert!(content.ends_with('\n'));
        assert!(content.contains("\"token\": \"token\""));
    }

    #[test]
    fn terminal_qr_renderer_returns_non_empty_text() {
        let rendered =
            render_onboarding_terminal_qr("https://example.com/login").expect("render qr");

        assert!(!rendered.trim().is_empty());
        assert!(rendered.contains('█'));
        assert!(rendered.contains('▀') || rendered.contains('▄'));
        assert!(rendered.lines().count() > 2);
    }

    #[tokio::test]
    async fn reuse_existing_false_runs_login_even_if_file_exists() {
        let dir = tempfile::tempdir().expect("tempdir");
        let credential_path = dir.path().join("wechat-credentials.json");
        std::fs::write(&credential_path, "old").expect("write credentials");
        let calls = Arc::new(AtomicUsize::new(0));
        let calls_for_login = Arc::clone(&calls);

        let result =
            ensure_wechat_onboarding_credentials_with_login(&credential_path, false, move || {
                calls_for_login.fetch_add(1, Ordering::SeqCst);
                async { Ok(test_credentials()) }
            })
            .await
            .expect("onboarding succeeds");

        assert!(!result.reused_existing_credentials);
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        let content = std::fs::read_to_string(&credential_path).expect("credentials written");
        assert!(content.contains("\"accountId\": \"account\""));
    }

    #[tokio::test]
    async fn parent_directories_created() {
        let dir = tempfile::tempdir().expect("tempdir");
        let credential_path = dir
            .path()
            .join("nested")
            .join("dir")
            .join("credentials.json");

        ensure_wechat_onboarding_credentials_with_login(&credential_path, true, || async {
            Ok(test_credentials())
        })
        .await
        .expect("onboarding succeeds");

        assert!(credential_path.is_file());
    }
}
