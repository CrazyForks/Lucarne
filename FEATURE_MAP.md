# Lucarne 功能全景

> AI Agent 多路复用守护进程：把 Claude、Codex、Gemini、Copilot、Pi 等 agent CLI 的私有协议统一成标准接口，上游（Telegram、微信）通过单一通道驱动多个 agent。

---

## 一、微信端功能点（`lucarne-wechat`）

### 1.1 登录与接入

- 终端 Unicode 二维码扫码登录
- 登录态持久化到 `~/.lucarned/wechat-credentials.json`，跨重启复用
- 支持强制重新登录（`LUCARNE_WECHAT_FORCE_LOGIN` / `channels.wechat.force_login`）
- 独立 onboarding 流程：先拿凭证，再启适配器
- 支持自动启用（`force_login`、`notify_user_ids` 非空、或凭证文件已存在）
- 自定义 iLink app ID、route tag、base URL、bot user-agent

### 1.2 消息收发

- 接收微信消息（文本 + 引用回复），提取 message_id、quoted_message_id、quoted_text
- 发送/回复消息：长文本自动分片，每片独立绑定
- Markdown 过滤：剥离微信不支持的语法（`LUCARNE_WECHAT_MARKDOWN_FILTER`，默认开）
- 引用路由双策略：
  - 主路径：通过 `message_id` 匹配
  - 兜底：对引用文本做 FNV-1a 哈希匹配（微信有时丢失引用 ID）
- 用户主动发消息时记入通知用户池

### 1.3 斜杠命令（6 个）

| 命令 | 作用 | 作用域 |
|---|---|---|
| `/status` | 查看全局 agent 资源快照（无引用）或单 workspace 详细状态（引用通知） | Global / Workspace |
| `/kill all` | 杀掉所有 agent 进程 | Global / Workspace |
| `/kill <session_id:pid>` | 杀指定进程 | Global / Workspace |
| `/config` | 查看当前 bypass、notifications 状态 | Global |
| `/config global bypass on\|off` | 全局权限绕过开关 | Global |
| `/config global notifications on\|off` | 全局通知开关 | Global |

### 1.4 会话管理

- 引用回复通知 → 继续对话。自动查找绑定的 provider session，必要时 resume
- 无引用的纯文本 → 回复提示"请引用通知继续对话"
- 过期引用 → 回复"该通知已不可路由"
- 全局 bypass 模式：开则 resume 时跳过权限检查

### 1.5 通知系统

- 通知推送目标：`notify_user_ids` config + 运行时记住的用户 ID
- 通知最多排队 10 条（`MAX_PENDING_NOTIFICATIONS`）；超限淘汰最旧
- 通知策略：全局开关 + 单 workspace 可单独关闭
- 活跃对话期间抑制该 workspace 的重复通知（`direct_notification_suppression`）
- Agent 通知渲染：markdown 正文 + footer（session ref、cwd）

### 1.6 排队与重试

- 待回复队列（最多 10 条 `MAX_PENDING_REPLIES`）：FIFO，超限淘汰最旧
- 待通知队列（最多 10 条）
- 传输失败 → 保留并周期重试（默认 5s 间隔）
- 频率限制：遇到 `RateLimited` → 记录退避截止时间；用户发消息立即清除退避

### 1.7 输入状态（Typing Keepalive）

- 收到消息后启动 `send_typing` 心跳（4s 间隔），agent 出结果后停止
- 每个 workspace 独立，新 turn 取消旧 keepalive
- 发送 typing 失败不影响用户消息处理

### 1.8 状态详情渲染（`/status` 引用通知）

```
版本 model(detail, reasoning) 权限 目录
agents.md 账号 base_url proxy 配置来源
session_id
token: input 1.2k · output 3.4k
context: 45k / 200k (22%) · 3 compactions
进程: pid 12345 · cpu 12% · mem 234 MiB
live 状态 · channel 绑定状态
```

### 1.9 上下文管理（SQLite）

- Schema：`lucarne_wechat_contexts`（account_key + user_id + context_token + observed_at + disabled）
- 持久化存储，跨重启保持
- 账号认证过期 → `disable_account` 标记所有 context 为 disabled
- 新 context 观察 → 自动重置 `disabled=0`（恢复）
- 按 user 查最新 enabled context（多账号去重）
- 轮询游标持久化（`lucarne_wechat_cursors`）

### 1.10 上下文过期提醒

| 配置项 | 默认值 |
|---|---|
| `expires_after` | 7200s（2 小时） |
| `remind_before` | 300s（5 分钟） |
| `prompt_template` | "会话将在 {remaining_minutes} 分钟后到期，请引用本条通知继续对话以保持会话。" |

### 1.11 频率限制交互

- 接近发送配额时推送可配置提醒文案
- 默认中文提示

---

## 二、Telegram 端功能点（`lucarne-telegram`）

### 2.1 登录与接入

- Bot Token 验证（`getMe`）+ 自动发现 chat（`getUpdates` 扫描候选）
- 需显式启用：`LUCARNE_TELEGRAM_ENABLED=true` + `TELEGRAM_BOT_TOKEN`
- 连接测试：启动时向 entry chat 发送 `"✓ lucarne online"`
- 用户授权白名单：`LUCARNE_AUTHORIZED_USER_IDS`（逗号分割，空 = 所有人）

### 2.2 入口面板（`/panel` `/start` `/refresh`）

三种视图，面板 snapshot 含 revision 防抖：

| 视图 | 内容 |
|---|---|
| **Overview** | agents（含 "New" 按钮）+ 分页历史记录 + 已绑定 sessions |
| **Workspaces** | 按项目目录浏览，可切换 |
| **Sessions** | 按 provider 过滤的 session 历史，分页（每页 5 项） |

面板交互：
- 翻页：inline 按钮（带 `N-N / total`）；`/next` `/prev` 保留为隐藏兼容输入，不注册 BotCommand
- 快捷索引：`/aN`（agent）、`/hN`（history）、`/wN`（workspace）
- 按钮操作：view switch、provider filter、workspace filter、new workspace、config toggle
- 面板 revision 追踪：过期点击静默忽略

### 2.3 Bot menu 命令 + 快捷命令

| 命令 | 作用 | 作用域 |
|---|---|---|
| `/start` | 打开管理面板 | Entry |
| `/panel` `/start` | 刷新/打开面板 | Entry |
| `/help` | 命令帮助 | Entry / Topic |
| `/config` | 查看/设置配置；Entry/通知 topic 为 global，workspace topic 支持 workspace/session scope | Entry / Topic / Notification |
| `/status` | Entry 查看全局资源；Topic 查看当前 workspace agent 状态（含进程资源） | Entry / Topic |
| `/kill all\|<session_id:pid>` | Entry 全局 kill；Topic 限定当前 workspace kill | Entry / Topic |
| `/clear_workspaces` | 清空 workspace 记录 | Entry |
| `/reset_notifications` | 重建通知 topic | Entry / Notification |
| `/aN` | 用面板第 N 个 agent 新建 session | Entry shortcut |
| `/hN` | 恢复当前页第 N 条历史 session | Entry shortcut |
| `/wN` | 打开当前视图第 N 个 workspace | Entry shortcut |
| `/rename <name>` | 重命名当前 workspace | Topic |
| `/commands` | 列出 agent 命令 | Topic |
| `/commands <command>` | 通过 Lucarne 调用 agent 命令 | Topic |
| `/commands <command> help` | 查看命令帮助 | Topic |
| `/model [model] [reasoning]` `/models` | 查看/切换模型和推理档位 | Topic |
| `/permissions [mode]` | 查看/设置权限 | Topic |
| `/skills` | 列出可用 skills | Topic |
| `/interrupt` | 中断当前 turn（绕队） | Topic |
| `/new` | 新建对话 | Topic |
| `/quit` | 关闭 live session | Topic |
| `/fork [target]` | 列 fork 目标或 fork 指定目标 | Topic |
| `/fN` | fork `/fork` 列表中的第 N 个目标 | Topic shortcut |

隐藏兼容输入：`/refresh`、`/next`、`/prev` 仍可手输或由按钮路径触发，但不注册到 Telegram BotCommand，也不作为公开命令展示。

### 2.4 会话机制

- Workspace = Telegram Forum Topic
- 延迟绑定：topic 创建时不启动 agent，用户首次发消息时才 `ensure_live_bound`
- 懒 hydrate：消息来时查不到绑定 → 从控制面恢复
- 未绑定 topic → 回复 "isn't bound to an agent session"
- 意外 topic 清理：跟踪 `recent_unbound_topic_creations`，删除入口命令误创建的 topic

### 2.5 Turn 调度

- 单 workspace 排队：`/interrupt` 绕过，其余 FIFO 等待 turn slot
- 排队位置提示："⏳ queued · position N"
- 闲置超时（`turn.inactivity_secs`）：1800s (30min) 无事件 → TurnFailed
- 绝对截止（`turn.deadline_secs`）：3600s (1h)
- 提交前清空前一 turn 遗留事件（`drain_stale_events`）

### 2.6 实时状态 Ticker

- 每 1200ms 编辑状态消息：`"⏳ 处理中 · Ns · M steps"`
- 30s 无事件追加 `"(等待 agent 输出)"`
- 80 字符 activity 摘要（tool call 名称、reasoning 片段等）
- 完成/失败后编辑为最终状态

### 2.7 Markdown 渲染

- Telegram MarkdownV2 专有渲染器
- 4000 字符硬拆行 + 内联键盘附在最后一片
- 编辑消息：`MessageNotModified` 视为幂等
- 解析失败（`FormatRejected`）→ 降级纯文本重试
- 编辑时 payload 超限 → 截断 + `…(truncated)` 标记
- 最终回复 footer：cost、duration、workspace 信息

### 2.8 通知系统

- 专用 "agent notifications" forum topic
- 从非活跃 session 的 agent 输出 → 通知 topic
- 通知渲染：workspace title + provider + resume ref
- 通知引用回复 → 原 session 路由（`message_session_binding`）
- 通知 topic 丢失时自动修复
- `/reset_notifications` 强制重建通知 topic
- 活跃 turn 期间 `DirectNotificationGuard` 抑制直接投递

### 2.9 干预处理（Approve / Deny / Answer）

- Agent 工具审批 → inline 按钮 `[Approve]` `[Deny]`
- 提问（AskUserQuestion）→ 多选 / freeform 答案按钮
- 校验：callback token、workspace 匹配、live instance 身份
- 处理完毕：删除干预提示 + 短暂闪现 ack 消息（1200ms 后自删）
- 过期 callback → 静默忽略

### 2.10 附件处理

- 图片：photo + document（最大 20MB）→ 下载 → base64 编码 → 多模态输入
- 纯图片无文字 → 暂存 `pending_images`，等下次文本消息时合入
- 文本附件 → `lucarne_channel::ingest` 摄入

### 2.11 Inline Query

- 在输入框输入 `/` 触发命令自动补全
- 按用户记上次使用的 workspace → 用对应 agent 的命令目录
- 最多返回 50 个补全项

### 2.12 历史回放

- 从历史入口打开 → 回放最近批次到 topic
- `HISTORY_REPLAY_LIMIT` 条消息，区分 user/assistant 角色和 turn 边界
- 恢复的 session 自动 resume
- "Older" 按钮 → 请求更早历史

### 2.13 Channel 操作

| 操作 | 说明 |
|---|---|
| `create_workspace` | 创建 forum topic |
| `rename_workspace` | 重命名 topic |
| `delete_workspace` | 删除 topic |
| `probe_workspace` | 发包 typing 验证 topic 存在 |
| `send / send_all` | 发送消息（自动拆行 4000 字符） |
| `edit` | 编辑消息 |
| `delete` | 删除消息 |
| `send_file` | 发送文件（caption 支持） |
| `download_attachment` | 下载附件到 buffer |
| `sync_commands` | 推送 BotCommand 列表到 Telegram |
| `answer_command_query` | 回答 inline query |

---

## 三、测试覆盖的用户旅程

> 总计 ~265+ 测试函数，覆盖 67 个编号 journey，分布在 39 个测试文件中。

### 3.1 Agent 会话生命周期

| 场景 | 说明 |
|---|---|
| 打开会话 | `open` → 初始 prompt → 收到 agent 响应 → `SessionClosed` |
| 多轮对话 | 连续发送、接收，session 持续 |
| 恢复会话 | `close` → 拿到 `SessionRef` → `resume` → 上下文延续 |
| 怠速超时 | idle 超时自动 close；可禁用（`idle_timeout_ms=0`） |
| 并发会话 | 单 runtime 管理多个并发 session，事件不窜 |
| 会话关闭 | `close` 命令、close 期间拒绝后续命令、重复 close 拒绝 |
| 带图片打开 | 多模态 input 正确编码并提交 |

### 3.2 通信与控制

| 场景 | 说明 |
|---|---|
| Turn 提交 | 文本 prompt → agent 处理 → `TurnCompleted`（携带 usage） |
| 排队 | busy 时提交 = 排队等下一轮；排队位置提示 |
| 中断 | `interrupt` 发送 → 中断确认；中断后保留工具完成结果 |
| 命令执行 | `/model` `/fork` `/status` `/permissions` `/skills` `/commands` `/new` `/quit` |
| 命令落地 | 所有 provider（claude/codex/gemini/pi/copilot）命令路由到正确实现 |
| 未知命令 | 拒绝而非 fallback 为 prompt |
| 双通道 | Telegram + WeChat 同时投递、活跃状态共享、live session 复用 |

### 3.3 推理与思维流

| 场景 | 说明 |
|---|---|
| 推理文本 | `ReasoningEvent` 流式推送 thinking/reasoning 文本 |
| 内部工具与日志 | `askUserQuestion` 内部工具不在 timeline 曝露，log 行被过滤 |

### 3.4 工具与审批

| 场景 | 说明 |
|---|---|
| 工具流 | agent 调用工具 → `ToolCall` 事件 → `ToolResult` 事件 → assistant 文本 |
| 工具失败 | 工具执行失败 → 错误 result → `TurnFailed` 或恢复 |
| 审批允许 | agent 请求权限 → 用户 `Allow` → 工具执行 |
| 审批拒绝 | agent 请求权限 → 用户 `Deny` → 工具被拒 |
| 风险操作拒绝 | delete / file change denied → `declined` / `rejected` tool result |
| 问题流 | agent 提问（单/多选、自由文本）→ 用户回答 → turn 继续 |
| 对话式权限 | Claude 非结构化权限 → 重新审批循环 |
| 子代理 | Task tool call → 子 agent identity metadata 保留 → 子 agent 按钮 |
| 结构化 vs 对话式 | `AgentCapabilities` 决定干预走结构化还是对话式通道 |

### 3.5 Fork 流

| 场景 | 说明 |
|---|---|
| 列目标 | `list_fork_targets` → 返回候选（标签、session ref、source 信息） |
| 选择 | 用户 `fork(N)` → 创建子 workspace → 新 session 绑定 → `forked` 确认 |
| 不绑源 | fork 不改变源 workspace channel 绑定 |
| 无候选 | 无 fork target 时不注册可选项 |
| Pi 特殊 | 未持久化 session 只在 live 期内可 fork |

### 3.6 多模态输入

- 图片 + 文字 → 编码为 Claude 格式 base64 content block
- 纯图片（空文本）→ 有效
- Telegram：图片暂存等下次文字合并
- WeChat：无多模态支持

### 3.7 Provider 特性矩阵

| 特性 | Claude | Codex | Gemini | Copilot | Pi |
|---|---|---|---|---|---|
| 推理 (thinking) | ✅ | ✅ | ✅ | ✅ | ✅ |
| 工具调用 | ✅ | ✅ | ✅ | ✅ | ✅ |
| 结构化审批 | ✅ | ✅ | ✅ | — | ✅ |
| AskUserQuestion | ✅ | ✅ | ✅ | — | — |
| 使用量追踪 | ✅ | ✅ | ✅ | ✅ | ✅（含 cost_usd） |
| 中断 | ✅ | ✅ | ✅ | — | ✅ |
| Resume | ✅ | ✅ | ✅ | — | ✅ |
| 子代理 | ✅ | ✅ | — | — | — |
| 原生命令 | ✅ | ✅ | ✅ | — | ✅（RPC） |
| Fork | ✅ | ✅ | — | — | ✅ |

说明：Codex 支持 reasoning/commentary 投影和 collab/sub-agent timeline；Codex 权限/提问走结构化 approval/question。

### 3.8 消息投递与路由

**Telegram：**
- workspace↔topic 双向绑定（SQLite 控制面 + 内存 cache）
- fork 重新绑定当前 topic
- 通知 topic 回复路由（`message_session_binding`）
- inline query 命令补全（按用户上次 workspace）
- entry panel 分页、revision 防抖
- 同一 topic ID 不同 chat → 隔离 session

**WeChat：**
- 引用路由：messageId → 文本哈希兜底
- 分片绑定：长文本每片独立绑定（引用任一片都能路由）
- markdown 过滤后引用文本仍能匹配哈希
- context token 跨重启持久化

### 3.9 持久化

| 层面 | 存储 |
|---|---|
| 控制面 | SQLite：workspace、provider_session、live_instance、turn、timeline（index + payload）、notification topic handle、channel binding |
| WeChat context | SQLite 自有表：context token + 轮询游标 |
| 历史索引 | 文件扫描 + metadata cache + 分页（RwLock 读优化） |
| 历史转录 | SessionReader byte cursor，热路径 bound reads |

### 3.10 通知与抑制

- 通知 topic 独立管理（Telegram）
- 直接投递抑制：活跃 turn 期间不出重复通知（跨通道共享）
- 全局通知开关 + workspace 级别覆盖
- `/config global notifications on|off`（微信）
- 通知队列 bounded（10），超限 FIFO 淘汰

### 3.11 故障与恢复

| 场景 | 行为 |
|---|---|
| 传输失败 | pending reply/notification 保留 + 周期重试（5s） |
| 频率限制 | `RateLimited` → 退避至 `retry_after`；用户消息清除退避 |
| 连接断开 | broken pipe → `TurnFailed`（含错误文本） |
| Idle 超时 | 1800s (30min) 无事件 → `TurnFailed` |
| Turn 截止 | 3600s (1h) 绝对 → `TurnFailed` |
| Topic 丢失 | 通知 topic 自动修复 |
| 过期引用 | WeChat stale quote → "no longer routable" |
| 会话过期 | 认证过期 → 全部 context disabled；新 context → 自动恢复 |
| Resume 失败 | Codex 回退 fresh start；其他 provider 报错 |
| Filter 变更 | 排空 backlog 后切换，不丢事件 |

### 3.12 设计约束（源级验证，63+ 断言）

**Provider 边界：**
- Provider 责任不泄露到公共层
- 无 provider 名称硬编码在 history / index
- 无 generic raw parser bridge（`ProviderParser → ProviderParsed<Body>`）
- `agent-sessions` crate root 无遗留 raw API

**性能与并发：**
- 状态锁外做 SQLite 写（`core_service_does_not_write_sqlite_while_holding_state_mutex`）
- 状态缓存 `RwLock` 读优化，非 `Mutex`
- 热路径 bound reads / windows，无全文件扫描
- RunTimeBus 异步锁用于 stdin/dialect，同步锁用于 metadata
- Channel 事件接收器 `std::sync::Mutex`，非 `tokio::sync::Mutex`

**依赖隔离：**
- `lucarne-adapter` 零 channel/platform 依赖
- Telegram 生产代码不直接开 session/DB，走 Core API
- adapter plugin 通过 trait 注册，无硬编码列表

**可观测性：**
- 所有生产模块 emit 结构化 tracing（span + event）
- 内存性能 snapshot 在启动各阶段

**数据流：**
- `AgentInput` 提交时 move，不 clone
- `WorkspaceProjectPath` move，不 clone
- `ReconcileOutcome` 是 `Copy`
- `ResumeRef` 保持 `SmolStr` 直到 UI 边界

---

## 四、架构概览

```
┌─────────────┐  ┌─────────────┐
│  Telegram   │  │   WeChat    │  ← 用户接触面
└──────┬──────┘  └──────┬──────┘
       │                │
   lucarne-         lucarne-
   telegram         wechat          ← Channel adapter（命令、通知、队列、重试）
       │                │
       └───────┬────────┘
          lucarne-adapter           ← Plugin registry
               │
           lucarne                  ← Core: runtime bus, control plane, history, daemon
               │
         agent-sessions             ← Provider parse / discovery / watch
               │
    ┌──────┬──────┬──────┬──────┐
  Claude  Codex Gemini Copilot  Pi  ← Agent CLI 进程
```

---

## 五、统一超时配置

```yaml
turn:
  inactivity_secs: 1800   # 30min，无事件则 TurnFailed
  deadline_secs: 3600     # 1h，turn 绝对上限

session:
  idle_timeout_secs: 7200 # 2h，agent session 闲置自动关闭
```

同一套 core 配置同时影响 Telegram 和 WeChat；Telegram 不再维护私有 turn timeout。

---

## 六、关键数字

| 指标 | 值 |
|---|---|
| 测试函数总数 | ~265+ |
| 编号 Journey | 67（全部 Covered） |
| 斜杠命令（Telegram） | 20 |
| 斜杠命令（WeChat） | 6 |
| 支持的 Agent Provider | 5（Claude、Codex、Gemini、Copilot、Pi） |
| 消息字符限制（Telegram） | 4000 |
| 通知队列上限 | 10 |
| 待回复队列上限 | 10 |
| Typing 心跳间隔 | 4s |
| Turn 闲置超时 | 1800s (30min) |
| Turn 绝对截止 | 3600s (1h) |
| Session idle timeout | 7200s (2h) |
| 面板分页大小 | 5 |
| Inline query 结果上限 | 50 |
| 图片大小上限 | 20MB |
