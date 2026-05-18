use std::{collections::BTreeMap, error::Error, fmt};

use serde::Deserialize;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TelegramOnboardingBot {
    pub username: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TelegramOnboardingChat {
    pub id: i64,
    pub label: String,
}

#[derive(Clone)]
pub struct TelegramOnboardingClient {
    http_client: reqwest::Client,
}

impl TelegramOnboardingClient {
    pub fn new(http_client: reqwest::Client) -> Self {
        Self { http_client }
    }

    pub async fn validate_token(
        &self,
        token: &str,
    ) -> Result<TelegramOnboardingBot, Box<dyn Error>> {
        let bytes = self
            .http_client
            .get(format!("https://api.telegram.org/bot{token}/getMe"))
            .send()
            .await
            .map_err(without_reqwest_url)?
            .bytes()
            .await
            .map_err(without_reqwest_url)?;

        parse_get_me(&bytes)
    }

    pub async fn discover_chats(
        &self,
        token: &str,
    ) -> Result<Vec<TelegramOnboardingChat>, Box<dyn Error>> {
        let bytes = self
            .http_client
            .get(format!("https://api.telegram.org/bot{token}/getUpdates"))
            .query(&[("timeout", "1")])
            .send()
            .await
            .map_err(without_reqwest_url)?
            .bytes()
            .await
            .map_err(without_reqwest_url)?;

        parse_chat_candidates(&bytes)
    }
}

fn without_reqwest_url(err: reqwest::Error) -> Box<dyn Error> {
    Box::new(err.without_url())
}

fn parse_get_me(bytes: &[u8]) -> Result<TelegramOnboardingBot, Box<dyn Error>> {
    let response: TelegramResponse<GetMeResult> = serde_json::from_slice(bytes)?;
    response.into_result().map(|result| TelegramOnboardingBot {
        username: result.username,
    })
}

fn parse_chat_candidates(bytes: &[u8]) -> Result<Vec<TelegramOnboardingChat>, Box<dyn Error>> {
    let response: TelegramResponse<Vec<Update>> = serde_json::from_slice(bytes)?;
    let mut chats = BTreeMap::new();

    for update in response.into_result()? {
        for chat in update.chats() {
            chats
                .entry(chat.id)
                .or_insert_with(|| TelegramOnboardingChat {
                    id: chat.id,
                    label: chat.label(),
                });
        }
    }

    Ok(chats.into_values().collect())
}

#[derive(Debug, Deserialize)]
struct TelegramResponse<T> {
    ok: bool,
    result: Option<T>,
    description: Option<String>,
}

impl<T> TelegramResponse<T> {
    fn into_result(self) -> Result<T, Box<dyn Error>> {
        if self.ok {
            self.result.ok_or_else(|| {
                Box::new(TelegramApiError("missing Telegram result".to_string())) as Box<dyn Error>
            })
        } else {
            Err(Box::new(TelegramApiError(
                self.description
                    .unwrap_or_else(|| "Telegram API error".to_string()),
            )))
        }
    }
}

#[derive(Debug, Deserialize)]
struct GetMeResult {
    username: Option<String>,
}

#[derive(Debug, Deserialize)]
struct Update {
    message: Option<Message>,
    edited_message: Option<Message>,
    channel_post: Option<Message>,
    edited_channel_post: Option<Message>,
}

impl Update {
    fn chats(&self) -> impl Iterator<Item = &Chat> {
        [
            self.message.as_ref(),
            self.edited_message.as_ref(),
            self.channel_post.as_ref(),
            self.edited_channel_post.as_ref(),
        ]
        .into_iter()
        .flatten()
        .map(|message| &message.chat)
    }
}

#[derive(Debug, Deserialize)]
struct Message {
    chat: Chat,
}

#[derive(Debug, Deserialize)]
struct Chat {
    id: i64,
    #[serde(rename = "type")]
    kind: String,
    title: Option<String>,
    username: Option<String>,
    first_name: Option<String>,
    last_name: Option<String>,
}

impl Chat {
    fn label(&self) -> String {
        let mut parts = Vec::new();

        if let Some(title) = self.title.as_deref().and_then(non_empty) {
            parts.push(title.to_string());
        }
        if let Some(username) = self.username.as_deref().and_then(non_empty) {
            parts.push(format!("@{username}"));
        }

        let full_name = [self.first_name.as_deref(), self.last_name.as_deref()]
            .into_iter()
            .flatten()
            .filter_map(non_empty)
            .collect::<Vec<_>>()
            .join(" ");
        if !full_name.is_empty() {
            parts.push(format!("({full_name})"));
        }

        if parts.is_empty() {
            format!("{} unnamed chat", self.kind)
        } else {
            format!("{} {}", self.kind, parts.join(" "))
        }
    }
}

fn non_empty(value: &str) -> Option<&str> {
    let value = value.trim();
    (!value.is_empty()).then_some(value)
}

#[derive(Debug, Eq, PartialEq)]
struct TelegramApiError(String);

impl fmt::Display for TelegramApiError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl Error for TelegramApiError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_get_me_response() {
        let bot = parse_get_me(
            br#"{
                "ok": true,
                "result": { "id": 123, "is_bot": true, "username": "lucarne_bot" }
            }"#,
        )
        .expect("getMe should parse");

        assert_eq!(
            bot,
            TelegramOnboardingBot {
                username: Some("lucarne_bot".to_string()),
            }
        );
    }

    #[test]
    fn parses_chat_candidates_from_updates() {
        let chats = parse_chat_candidates(
            br#"{
                "ok": true,
                "result": [
                    { "update_id": 1, "message": { "chat": { "id": 42, "type": "private", "username": "era", "first_name": "Era" } } },
                    { "update_id": 2, "message": { "chat": { "id": 42, "type": "private", "username": "era", "first_name": "Era" } } },
                    { "update_id": 3, "message": { "chat": { "id": -100, "type": "supergroup", "title": "Lucarne Ops" } } }
                ]
            }"#,
        )
        .expect("updates should parse");

        assert_eq!(
            chats,
            vec![
                TelegramOnboardingChat {
                    id: -100,
                    label: "supergroup Lucarne Ops".to_string(),
                },
                TelegramOnboardingChat {
                    id: 42,
                    label: "private @era (Era)".to_string(),
                },
            ]
        );
    }

    #[test]
    fn telegram_api_error_becomes_error() {
        let err = parse_get_me(br#"{ "ok": false, "description": "Unauthorized" }"#)
            .expect_err("telegram errors should become parser errors");

        assert_eq!(err.to_string(), "Unauthorized");
    }

    #[test]
    fn unnamed_chat_label_does_not_panic() {
        let chats = parse_chat_candidates(
            br#"{
                "ok": true,
                "result": [
                    { "update_id": 1, "message": { "chat": { "id": 7, "type": "private" } } }
                ]
            }"#,
        )
        .expect("updates should parse");

        assert_eq!(
            chats,
            vec![TelegramOnboardingChat {
                id: 7,
                label: "private unnamed chat".to_string(),
            }]
        );
    }
}
