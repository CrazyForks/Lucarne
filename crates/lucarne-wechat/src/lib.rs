//! WeChat notification bridge for lucarne.
//!
//! This crate intentionally implements only the WeChat user journey:
//! watched agent messages are delivered to WeChat users, quoted replies
//! continue the bound provider session, and scoped notification policy
//! is resolved by `LucarneCore`.

pub mod adapter;
pub mod context_store;
pub mod onboarding;
pub mod service;

pub use adapter::{run_wechat_adapter, wechat_plugin, WechatAdapterPlugin, WechatConfig};
pub use service::{
    WechatError, WechatIncoming, WechatNotificationService, WechatSendReceipt,
    WechatServiceOptions, WechatTransport, WechatUserInteractionRequest,
};
