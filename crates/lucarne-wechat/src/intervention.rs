use lucarne::{
    ApprovalDecision, ApprovalRequest, InterventionRequest, InterventionResponse, Question,
    QuestionAnswer, QuestionResponse,
};
use smol_str::SmolStr;

const MAX_JSON_CHARS: usize = 1200;
const MAX_MESSAGE_CHARS: usize = 400;

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct ParsedInterventionTextResponse {
    pub response: InterventionResponse,
    pub ack_markdown: String,
}

pub(crate) fn render_intervention_markdown_zh(request: &InterventionRequest) -> String {
    match request {
        InterventionRequest::Approval(request) => render_approval_markdown_zh(request),
        InterventionRequest::Question(request) => {
            let mut body = String::from("## 需要你回答\n\n");
            for (index, question) in request.questions.iter().enumerate() {
                if index > 0 {
                    body.push('\n');
                }
                append_question_markdown_zh(&mut body, index, question);
            }
            if request.questions.is_empty() {
                body.push_str("Agent 需要输入，但没有提供具体问题。\n");
            }
            body
        }
    }
}

pub(crate) fn parse_intervention_text_response_zh(
    request: &InterventionRequest,
    text: &str,
) -> Result<ParsedInterventionTextResponse, String> {
    match request {
        InterventionRequest::Approval(_) => parse_approval_response_zh(text),
        InterventionRequest::Question(request) => parse_question_response_zh(request, text),
    }
}

pub(crate) fn intervention_request_id(request: &InterventionRequest) -> &SmolStr {
    match request {
        InterventionRequest::Approval(request) => &request.req_id,
        InterventionRequest::Question(request) => &request.req_id,
    }
}

fn render_approval_markdown_zh(request: &ApprovalRequest) -> String {
    let mut body = format!(
        "## 需要授权\n\n**工具**：`{}`\n",
        markdown_inline_code(request.tool_name.as_str())
    );
    if let Some(message) = request
        .message
        .as_deref()
        .map(str::trim)
        .filter(|message| !message.is_empty())
    {
        body.push_str(&format!(
            "\n**说明**：{}\n",
            short_markdown_text(message, MAX_MESSAGE_CHARS)
        ));
    }
    if let Some(input) = request.input.as_ref() {
        body.push_str("\n**参数**：\n");
        body.push_str("```json\n");
        body.push_str(&short_code_block(&json_pretty(input), MAX_JSON_CHARS));
        body.push_str("\n```\n");
    }
    body.push_str("\n回复 `允许` 继续，或回复 `拒绝` 取消。");
    body
}

fn append_question_markdown_zh(body: &mut String, index: usize, question: &Question) {
    let title = question
        .header
        .as_deref()
        .map(str::trim)
        .filter(|header| !header.is_empty())
        .unwrap_or("问题");
    body.push_str(&format!("### {}. {}\n", index + 1, title));
    let text = question.text.trim();
    if !text.is_empty() {
        body.push_str(text);
        body.push_str("\n\n");
    }
    if question.options.is_empty() {
        body.push_str("直接回复答案。\n");
        return;
    }
    for (option_index, option) in question.options.iter().enumerate() {
        let letter = option_letter(option_index);
        body.push_str(&format!(
            "- `{letter}` **{}**",
            markdown_bold_text(option.label.as_str())
        ));
        if let Some(description) = option
            .description
            .as_deref()
            .map(str::trim)
            .filter(|description| !description.is_empty())
        {
            body.push_str(&format!(" — {description}"));
        }
        body.push('\n');
    }
    if question.multi_select {
        body.push_str("\n可多选，空格或逗号分隔。回复示例：`A C`\n");
    } else {
        body.push_str("\n回复示例：`A`\n");
    }
}

fn parse_approval_response_zh(text: &str) -> Result<ParsedInterventionTextResponse, String> {
    let normalized = normalize_reply(text);
    let decision = if matches!(
        normalized.as_str(),
        "允许"
            | "同意"
            | "批准"
            | "通过"
            | "确认"
            | "继续"
            | "yes"
            | "y"
            | "ok"
            | "allow"
            | "approve"
    ) {
        ApprovalDecision::Allow
    } else if matches!(
        normalized.as_str(),
        "拒绝" | "否" | "不" | "不允许" | "取消" | "停止" | "no" | "n" | "deny" | "reject"
    ) {
        ApprovalDecision::Deny
    } else {
        return Err("请回复 `允许` 或 `拒绝`。".to_string());
    };
    let ack_markdown = match decision {
        ApprovalDecision::Allow => "已允许。",
        ApprovalDecision::Deny => "已拒绝。",
    }
    .to_string();
    Ok(ParsedInterventionTextResponse {
        response: InterventionResponse::Approval(decision),
        ack_markdown,
    })
}

fn parse_question_response_zh(
    request: &lucarne::QuestionRequest,
    text: &str,
) -> Result<ParsedInterventionTextResponse, String> {
    if request.questions.is_empty() {
        return Err("没有可回答的问题。".to_string());
    }
    let answers = if request.questions.len() == 1 {
        vec![QuestionAnswer {
            values: parse_single_question_values(&request.questions[0], text)?,
        }]
    } else {
        let lines = text
            .lines()
            .map(str::trim)
            .filter(|line| !line.is_empty())
            .collect::<Vec<_>>();
        if lines.len() != request.questions.len() {
            return Err(format!(
                "请按每行一个答案回复，共 {} 行。例如：\n1. A\n2. 文本",
                request.questions.len()
            ));
        }
        request
            .questions
            .iter()
            .zip(lines)
            .map(|(question, line)| {
                let answer = strip_question_line_prefix(line);
                parse_single_question_values(question, answer)
                    .map(|values| QuestionAnswer { values })
            })
            .collect::<Result<Vec<_>, _>>()?
    };
    Ok(ParsedInterventionTextResponse {
        response: InterventionResponse::Answers(QuestionResponse { answers }),
        ack_markdown: "已提交回答。".to_string(),
    })
}

fn parse_single_question_values(question: &Question, text: &str) -> Result<Vec<SmolStr>, String> {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return Err("回复不能为空。".to_string());
    }
    if question.options.is_empty() {
        return Ok(vec![trimmed.into()]);
    }
    let parts = split_answer_parts(trimmed);
    if !question.multi_select && parts.len() != 1 {
        return Err("这个问题只能选择一个选项。".to_string());
    }
    let mut values = Vec::new();
    for part in parts {
        let label = resolve_option_label(question, part)
            .ok_or_else(|| format!("无法识别选项 `{part}`。请回复 {}。", option_hint(question)))?;
        if !values.iter().any(|seen: &SmolStr| seen == &label) {
            values.push(label);
        }
    }
    Ok(values)
}

fn resolve_option_label(question: &Question, part: &str) -> Option<SmolStr> {
    let normalized = normalize_reply(part);
    if let Ok(number) = normalized.parse::<usize>() {
        if (1..=question.options.len()).contains(&number) {
            return Some(question.options[number - 1].label.clone());
        }
    }
    if normalized.chars().count() == 1 {
        let ch = normalized.chars().next()?;
        if ch.is_ascii_alphabetic() {
            let index = ch.to_ascii_uppercase() as usize - 'A' as usize;
            if let Some(option) = question.options.get(index) {
                return Some(option.label.clone());
            }
        }
    }
    question
        .options
        .iter()
        .find(|option| normalize_reply(option.label.as_str()) == normalized)
        .map(|option| option.label.clone())
}

fn split_answer_parts(text: &str) -> Vec<&str> {
    text.split(|ch: char| ch.is_whitespace() || matches!(ch, ',' | '，' | ';' | '；' | '+' | '、'))
        .map(str::trim)
        .filter(|part| !part.is_empty())
        .collect()
}

fn strip_question_line_prefix(line: &str) -> &str {
    line.trim_start()
        .trim_start_matches(|ch: char| ch.is_ascii_digit())
        .trim_start_matches(['.', '。', ':', '：', ')', '）', '-'])
        .trim()
}

fn option_hint(question: &Question) -> String {
    question
        .options
        .iter()
        .enumerate()
        .map(|(index, option)| format!("`{}`/`{}`", option_letter(index), option.label))
        .collect::<Vec<_>>()
        .join("、")
}

fn option_letter(index: usize) -> char {
    (b'A' + (index.min(25) as u8)) as char
}

fn normalize_reply(text: &str) -> String {
    text.trim()
        .trim_matches(|ch: char| matches!(ch, '.' | '。' | '!' | '！' | '?' | '？'))
        .trim()
        .to_ascii_lowercase()
}

fn json_pretty(value: &serde_json::Value) -> String {
    serde_json::to_string_pretty(value).unwrap_or_else(|_| value.to_string())
}

fn short_code_block(text: &str, max_chars: usize) -> String {
    if text.chars().count() <= max_chars {
        return text.to_string();
    }
    let prefix = text
        .chars()
        .take(max_chars.saturating_sub(1))
        .collect::<String>();
    format!("{prefix}…")
}

fn short_markdown_text(text: &str, max_chars: usize) -> String {
    if text.chars().count() <= max_chars {
        return text.to_string();
    }
    let prefix = text
        .chars()
        .take(max_chars.saturating_sub(1))
        .collect::<String>();
    format!("{prefix}…")
}

fn markdown_inline_code(text: &str) -> String {
    text.replace('`', "\\`")
}

fn markdown_bold_text(text: &str) -> String {
    text.replace("**", "＊")
}

#[cfg(test)]
mod tests {
    use super::*;
    use lucarne::{ApprovalRequest, QuestionOption, QuestionRequest};

    #[test]
    fn renders_approval_as_chinese_markdown_with_tool_input_and_reply_words() {
        let request = InterventionRequest::Approval(ApprovalRequest {
            req_id: "approval-1".into(),
            tool_name: "apply_patch".into(),
            message: Some("需要修改文件".into()),
            input: Some(serde_json::json!({ "cmd": "apply_patch", "path": "src/main.rs" })),
        });

        let markdown = render_intervention_markdown_zh(&request);

        assert!(markdown.contains("## 需要授权"), "{markdown}");
        assert!(markdown.contains("**工具**：`apply_patch`"), "{markdown}");
        assert!(markdown.contains("**说明**：需要修改文件"), "{markdown}");
        assert!(markdown.contains("```json"), "{markdown}");
        assert!(markdown.contains("\"path\": \"src/main.rs\""), "{markdown}");
        assert!(markdown.contains("回复 `允许`"), "{markdown}");
        assert!(markdown.contains("`拒绝`"), "{markdown}");
    }

    #[test]
    fn parses_chinese_approval_decisions() {
        let request = InterventionRequest::Approval(ApprovalRequest {
            req_id: "approval-1".into(),
            tool_name: "apply_patch".into(),
            message: None,
            input: None,
        });

        assert_eq!(
            parse_intervention_text_response_zh(&request, "允许").unwrap(),
            ParsedInterventionTextResponse {
                response: InterventionResponse::Approval(ApprovalDecision::Allow),
                ack_markdown: "已允许。".to_string(),
            }
        );
        assert_eq!(
            parse_intervention_text_response_zh(&request, "拒绝")
                .unwrap()
                .response,
            InterventionResponse::Approval(ApprovalDecision::Deny)
        );
        assert!(parse_intervention_text_response_zh(&request, "随便").is_err());
    }

    #[test]
    fn renders_and_parses_single_choice_question() {
        let request = InterventionRequest::Question(QuestionRequest {
            req_id: "question-1".into(),
            questions: vec![Question {
                header: Some("选择分支".into()),
                text: "要切到哪个分支？".into(),
                options: vec![
                    QuestionOption {
                        label: "main".into(),
                        description: Some("稳定分支".into()),
                    },
                    QuestionOption {
                        label: "feature".into(),
                        description: Some("功能分支".into()),
                    },
                ],
                multi_select: false,
            }],
        });

        let markdown = render_intervention_markdown_zh(&request);
        assert!(markdown.contains("## 需要你回答"), "{markdown}");
        assert!(markdown.contains("### 1. 选择分支"), "{markdown}");
        assert!(markdown.contains("- `A` **main** — 稳定分支"), "{markdown}");
        assert!(markdown.contains("回复示例：`A`"), "{markdown}");

        assert_eq!(
            parse_intervention_text_response_zh(&request, "B").unwrap(),
            ParsedInterventionTextResponse {
                response: InterventionResponse::Answers(QuestionResponse {
                    answers: vec![QuestionAnswer {
                        values: vec!["feature".into()],
                    }],
                }),
                ack_markdown: "已提交回答。".to_string(),
            }
        );
    }
}
