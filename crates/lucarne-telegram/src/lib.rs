//! Telegram channel + bot built on top of `lucarne` and `agent-sessions`.
//!
//! Layering:
//! * [`channel`] — a [`lucarne_channel::Channel`] implementation wrapping
//!   `frankenstein` (Bot API 10.1+ Rich Messages). Handles message splitting,
//!   rich markdown rendering, topic
//!   lifecycle, and inbound event translation.
//! * [`state`] — in-memory bot state (entry chat, topic↔instance map).
//! * [`agents`] — presentation entries for daemon-reported providers.
//! * [`history`] — re-export of core historical session enumeration.
//! * [`bot`] — flow glue: entry panel, working session, resume, rename.

pub mod adapter;
pub mod agents;
pub mod bot;
pub mod channel;
mod command_sync;
pub mod history;
pub mod onboarding;
pub mod state;
mod tg_bot;
pub mod turn;

pub use adapter::{
    run_telegram_adapter, run_telegram_adapter_with_client, telegram_plugin, TelegramAdapterPlugin,
};
