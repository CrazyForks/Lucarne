use lucarne::dialect::{Dialect, ImageRef, Input, OutFrame};
use lucarne::dialects::claude::Claude;
use serde_json::{json, Value};

#[test]
fn claude_encode_user_message_includes_base64_images() {
    let mut dialect = Claude::new();
    let frames = dialect
        .encode_user_message(&Input {
            text: "read the token".into(),
            images: vec![ImageRef {
                media_type: "image/png".into(),
                data: vec![1, 2, 3],
            }],
        })
        .expect("encode user message");

    assert_eq!(frames.len(), 1);
    let OutFrame::Stdin(bytes) = &frames[0] else {
        panic!("expected stdin frame: {frames:?}");
    };
    let payload: Value = serde_json::from_slice(bytes).expect("decode payload");
    assert_eq!(
        payload,
        json!({
            "type": "user",
            "message": {
                "role": "user",
                "content": [
                    {"type": "text", "text": "[Image #1]"},
                    {
                        "type": "image",
                        "source": {
                            "type": "base64",
                            "media_type": "image/png",
                            "data": "AQID"
                        }
                    },
                    {"type": "text", "text": "read the token"}
                ]
            }
        })
    );
}

#[test]
fn claude_encode_user_message_allows_image_only_inputs() {
    let mut dialect = Claude::new();
    let frames = dialect
        .encode_user_message(&Input {
            text: String::new(),
            images: vec![ImageRef {
                media_type: "image/png".into(),
                data: vec![1, 2, 3],
            }],
        })
        .expect("encode image-only input");

    let OutFrame::Stdin(bytes) = &frames[0] else {
        panic!("expected stdin frame: {frames:?}");
    };
    let payload: Value = serde_json::from_slice(bytes).expect("decode payload");
    assert_eq!(
        payload["message"]["content"],
        json!([
            {"type": "text", "text": "[Image #1]"},
            {
                "type": "image",
                "source": {
                    "type": "base64",
                    "media_type": "image/png",
                    "data": "AQID"
                }
            }
        ])
    );
}
