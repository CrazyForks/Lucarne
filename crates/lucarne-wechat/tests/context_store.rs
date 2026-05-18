use std::sync::{Arc, Mutex};

use lucarne_wechat::context_store::WechatContextStore;
use rusqlite::Connection;
use wechat_ilink::WechatContext;

fn sqlite_store() -> WechatContextStore {
    WechatContextStore::open(Arc::new(Mutex::new(Connection::open_in_memory().unwrap()))).unwrap()
}

#[tokio::test]
async fn context_store_persists_context_and_cursor_in_sqlite_private_tables() {
    let conn = Arc::new(Mutex::new(Connection::open_in_memory().unwrap()));
    let store = WechatContextStore::open(Arc::clone(&conn)).unwrap();

    store
        .upsert_context(WechatContext {
            account_key: "account-1".into(),
            user_id: "user-1".into(),
            context_token: "ctx-1".into(),
            observed_at_unix_ms: 10,
            source_message_id: Some("msg-1".into()),
        })
        .await
        .unwrap();
    store
        .upsert_context(WechatContext {
            account_key: "account-1".into(),
            user_id: "user-1".into(),
            context_token: "ctx-2".into(),
            observed_at_unix_ms: 30,
            source_message_id: Some("msg-2".into()),
        })
        .await
        .unwrap();
    store
        .save_cursor("account-1", "cursor-1".into(), 40)
        .await
        .unwrap();

    let restored = WechatContextStore::open(conn).unwrap();
    let context = restored
        .context("account-1", "user-1")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(context.context.context_token, "ctx-2");
    assert_eq!(
        restored.cursor("account-1").await.unwrap().as_deref(),
        Some("cursor-1")
    );
    let cursor = restored
        .cursor_record("account-1")
        .await
        .unwrap()
        .expect("cursor record");
    assert_eq!(cursor.account_key, "account-1");
    assert_eq!(cursor.updated_at_unix_ms, 40);
    assert_eq!(restored.all_contexts().await.unwrap().len(), 1);
}

#[tokio::test]
async fn context_observed_after_account_disable_reenables_context() {
    let store = sqlite_store();

    store
        .upsert_context(WechatContext {
            account_key: "account-1".into(),
            user_id: "user-1".into(),
            context_token: "ctx-1".into(),
            observed_at_unix_ms: 10,
            source_message_id: Some("msg-1".into()),
        })
        .await
        .unwrap();
    store.disable_account("account-1").await.unwrap();
    assert!(store
        .enabled_context_for_user("account-1", "user-1")
        .await
        .unwrap()
        .is_none());

    store
        .upsert_context(WechatContext {
            account_key: "account-1".into(),
            user_id: "user-1".into(),
            context_token: "ctx-2".into(),
            observed_at_unix_ms: 20,
            source_message_id: Some("msg-2".into()),
        })
        .await
        .unwrap();

    let enabled = store
        .enabled_context_for_user("account-1", "user-1")
        .await
        .unwrap()
        .expect("newly observed context should be enabled");
    assert_eq!(enabled.context_token, "ctx-2");
}
