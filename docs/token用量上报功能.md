# Token 用量上报功能 - 实施文档

> 更新日期: 2026-06-02 17:50
> 状态: ✅ Phase 1+2 完成 | 端到端测试通过 | UPSERT 去重 | project_name | username 配置 | Codex JSONL 数据源 | Cron 兜底 | 服务端自动计费

---

## 1. 已完成的工作

### 1.1 客户端（git-ai，Rust）

项目路径（更新文档时不允许删除，要保留）：/Users/xz/git-ai

项目文档（更新文档时不允许删除，要保留）：
- 原开源项目git-ai原始文档：/Users/xz/git-ai/README-old.md
- 基于开源项目做功能后的文档：/Users/xz/git-ai/README.md

#### 1.1.1 新增命令：`report-token-usage`

```bash
git-ai report-token-usage claude-code
git-ai report-token-usage codex
```

**实现文件：**

| 文件 | 职责 |
|------|------|
| `src/commands/report_token_usage/mod.rs` | 命令入口，组装 payload，HTTP 上报 |
| `src/commands/report_token_usage/claude.rs` | 读取 Claude Code 本地数据 |
| `src/commands/report_token_usage/codex.rs` | 读取 Codex 本地数据 |

**修改文件：**

| 文件 | 修改内容 |
|------|---------|
| `src/commands/mod.rs` | 添加 `pub mod report_token_usage;` |
| `src/commands/git_ai_handlers.rs` | 添加 `report-token-usage` 命令分支、跳过 daemon 初始化 |
| `src/mdm/agents/claude_code.rs` | Stop hook 安装/卸载 token 上报命令、hooks.json 自动管理（ECC 兼容） |
| `src/mdm/agents/codex.rs` | Stop hook 安装/卸载 token 上报命令 |
| `src/commands/tracker/config.rs` | 新增 `username` 字段 |

#### 1.1.2 各平台数据读取实现

**Claude Code (`claude.rs`):**
> 参考 [ccusage](https://github.com/ryoppippi/ccusage) 的数据读取方式。

- 数据源：`~/.claude/projects/<project>/<session_id>.jsonl`
  - 每个 `.jsonl` 文件代表一个 Claude 会话
  - 每行包含 `message.usage` 对象，字段：`input_tokens`, `output_tokens`, `cache_creation_input_tokens`, `cache_read_input_tokens`
  - 按文件聚合所有行的 token 数，返回最新修改的 session
  - 支持子 agent 文件：`<session_id>/subagents/<agent>.jsonl`

**Codex (`codex.rs`):**
> 参考 [ccusage](https://github.com/ryoppippi/ccusage) 的数据读取方式。

- 数据源：`~/.codex/sessions/**/*.jsonl`（JSONL 会话日志）
  - 每个 `.jsonl` 文件代表一个 Codex 会话
  - 解析 `token_count` 事件，提取 `total_token_usage`（累计值）和 `last_token_usage`（每 turn 增量）
  - 字段：`input_tokens`, `output_tokens`, `cached_input_tokens`, `reasoning_output_tokens`, `total_tokens`
  - 按会话聚合最新 token 数，自动识别最新修改的 session 文件
  - `model` 从 `turn_context` payload 中提取
  - ⚠️ Codex 的 `input_tokens` 包含 `cached_input_tokens`，上报时需拆分为非缓存 input 和 cache_read 分别上报

#### 1.1.3 Hook 安装

**Claude Code:** 在 `install_hooks_at` 中，除了现有的 PreToolUse/PostToolUse checkpoint 命令外，额外在 Stop hook 中安装：
```json
{
  "hooks": {
    "Stop": [{
      "matcher": "*",
      "hooks": [{
        "type": "command",
        "command": "/path/to/git-ai report-token-usage claude-code"
      }]
    }]
  }
}
```

**Codex:** 在 `hooks_with_installed_commands` 中，对 Stop 事件额外添加：
```json
{
  "hooks": {
    "Stop": [{
      "hooks": [
        { "type": "command", "command": "... checkpoint codex ..." },
        { "type": "command", "command": "... report-token-usage codex" }
      ]
    }]
  }
}
```

**卸载：** `uninstall_hooks_at`（Claude）和 `remove_codex_hooks_from_json`（Codex）都会清理 report-token-usage 命令。

#### 1.1.4 上报 Payload

```json
{
  "team_id": 1,
  "team_key": "your-team-key",
  "platform": "claude-code",
  "session_id": "e79a0918-5ed2-41de-9a36-73f7494e58c6",
  "model": "claude-sonnet-4-20250514",
  "username": "user@example.com",
  "input_tokens": 15000,
  "output_tokens": 3000,
  "cache_read_tokens": 12000,
  "cache_creation_tokens": 3000,
  "total_tokens": 33000,
  "cost_usd": 0.45,
  "project_name": "xm/demo",
  "repo_url": "https://github.com/org/repo.git",
  "reported_at": "2026-05-29T16:00:00Z"
}
```

### 1.2 后端（Spring Boot Java）

项目路径（更新文档时不允许删除，要保留）：/Users/xz/xm/srch-001-ai-tracker/backend，运行本地部署是：/Users/xz/xm/srch-001-ai-tracker/backend/deploy-local.sh，对应上报数据表是在后端的pg数据库中，数据表名称为：llm_token_usage

**实现文件：**

| 文件 | 职责 |
|------|------|
| `sql/init.sql`（v2.0 区块） | 数据库迁移脚本（已合并到 init.sql） |
| `sql/migrate_v1.3.sql` | 修复 cost_usd 列精度 DECIMAL(20,10)（2026-06-02） |
| `entity/LlmTokenUsage.java` | 实体类 |
| `mapper/LlmTokenUsageMapper.java` | MyBatis-Plus Mapper |
| `mapper/ModelPricingMapper.java` | 模型价格 Mapper（2026-06-02 新增 selectByModelNameIgnoreCase） |
| `controller/vo/TokenUsageReportReqVO.java` | 请求 VO |
| `controller/vo/TokenUsageRespVO.java` | 响应 VO |
| `service/TokenUsageReportService.java` | Service 接口 |
| `service/impl/TokenUsageReportServiceImpl.java` | Service 实现（2026-06-02 新增 calculateCost 自动计费） |
| `controller/TokenUsageReportController.java` | Controller |

**逻辑流程：**
1. 验证 `team_id` + `X-Team-Key`
2. 按 `username` 查找或自动创建 `employee`（name 默认为邮箱前缀）
3. 按 `session_id` + `platform` 查询已有记录：
   - **不存在** → 插入新记录
   - **已存在** → 比较 token 总量，新数据更大则 UPSERT 更新，否则跳过
4. **自动计费**（2026-06-02 新增）：
   - 用 `model` 不区分大小写精确匹配 `model_pricing.model_name`
   - 匹配到价格 → 按公式计算 `cost_usd = (input×input_price + output×output_price + cache_create×cache_create_price + cache_read×cache_read_price) / 1_000_000`
   - 先累加所有项再除以 1_000_000，仅在最后一步舍入（HALF_UP，10位小数）
   - 未匹配到价格 → `cost_usd` 设为 `null`，WARN 日志提示
5. 返回上报结果

---

## 2. 数据库设计

### 2.1 迁移脚本

> 已合并到 `sql/init.sql` 的 `-- v2.0 Token 用量上报` 区块，启动项目自动建表。

```sql
-- migrate_v2.0_token_usage.sql
CREATE TABLE IF NOT EXISTS llm_token_usage (
  id BIGSERIAL PRIMARY KEY,
  team_id BIGINT NOT NULL REFERENCES teams(id),
  employee_id BIGINT NOT NULL REFERENCES employees(id),
  platform VARCHAR(50) NOT NULL,          -- claude-code / codex / cursor
  session_id VARCHAR(255) NOT NULL,        -- 会话唯一标识
  model VARCHAR(100) NOT NULL,             -- 模型名称
  username VARCHAR(255) NOT NULL,           -- 用户邮箱

  -- Token 用量
  input_tokens BIGINT DEFAULT 0,
  output_tokens BIGINT DEFAULT 0,
  cache_read_tokens BIGINT DEFAULT 0,
  cache_creation_tokens BIGINT DEFAULT 0,
  total_tokens BIGINT GENERATED ALWAYS AS (
    input_tokens + output_tokens + cache_read_tokens + cache_creation_tokens
  ) STORED,

  -- 费用
  cost_usd DECIMAL(20, 10),

  -- 上下文
  repo_url VARCHAR(500),
  project_name VARCHAR(200),          -- 项目名称（可为空）
  reported_at TIMESTAMP NOT NULL,
  received_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP,

  -- 去重（同一个 session 多次上报会 UPSERT 更新）
  CONSTRAINT uk_session_platform UNIQUE (session_id, platform)
);

CREATE INDEX idx_token_usage_team_id ON llm_token_usage(team_id);
CREATE INDEX idx_token_usage_employee_id ON llm_token_usage(employee_id);
CREATE INDEX idx_token_usage_reported_at ON llm_token_usage(reported_at);
CREATE INDEX idx_token_usage_platform ON llm_token_usage(platform);
CREATE INDEX idx_token_usage_model ON llm_token_usage(model);
```

### 2.2 后端代码结构

```
backend/src/main/java/com/sproutlife/aicodeboost/
├── entity/
│   └── LlmTokenUsage.java          # 实体类
├── mapper/
│   └── LlmTokenUsageMapper.java    # MyBatis-Plus Mapper
├── controller/vo/
│   ├── TokenUsageReportReqVO.java  # 请求 VO
│   └── TokenUsageRespVO.java       # 响应 VO
├── controller/
│   └── TokenUsageReportController.java  # Controller
└── service/
    ├── TokenUsageReportService.java     # 接口
    └── impl/TokenUsageReportServiceImpl.java  # 实现
```

---

## 3. 待开发工作

### 3.1 后端（Java）

- [x] 创建数据库迁移脚本 → 已合并到 `sql/init.sql`（v2.0 区块）
- [x] 创建 Entity / Mapper / VO
- [x] 创建 Service 层（验证 team、关联 employee、UPSERT 更新）
- [x] 创建 Controller（`POST /ai-code-boost/open/report/token/usage`）
- [x] 添加 X-Team-Key 认证
- [x] curl 模拟上报测试通过 ✅
- [x] `reported_at` 时间格式修复（LocalDateTime → OffsetDateTime）

### 3.2 Cursor 支持

- [x] 调研 Cursor token 用量获取方案 → Cursor Codex 与原生 Codex 都写入 `~/.codex/sessions/**/*.jsonl`，共享同一数据源 ✅
- [x] 已实现：codex.rs 改读 JSONL 日志（参照 ccusage），cron 每分钟兜底上报（Cursor hooks 不支持 Stop 事件） ✅

---

## 4. 测试计划

### 4.1 客户端测试

```bash
# 1. 手动测试命令
git-ai report-token-usage claude-code
git-ai report-token-usage codex

# 2. 安装 hook
git-ai install-hooks

# 3. 验证 Claude Code settings.json 中的 Stop hook
cat ~/.claude/settings.json

# 4. 验证 Codex hooks.json 中的 Stop hook
cat ~/.codex/hooks.json

# 5. 运行一次 Claude Code 或 Codex 会话，验证 Stop hook 触发
```

### 4.2 后端测试

```bash
# 模拟上报
curl -X POST http://localhost:39527/ai-code-boost/open/report/token/usage \
  -H "Content-Type: application/json" \
  -H "X-Team-Key: backend-2024-def456" \
  -d '{
    "team_id": 2,
    "platform": "claude-code",
    "session_id": "test-session-1234",
    "model": "claude-sonnet-4-20250514",
    "username": "test@example.com",
    "input_tokens": 1000,
    "output_tokens": 500,
    "cache_read_tokens": 1000,
    "cache_creation_tokens": 500,
    "cost_usd": 2,
    "total_tokens": 1500,
    "reported_at": "2026-05-30T16:00:00Z"
  }'
```

**测试结果：** ✅ 后端测试通过

### 4.3 测试过程中修复的问题

1. **employees 表 name 非空约束**：`employees` 表的 `name` 列有 NOT NULL 约束，自动创建 employee 时未赋值导致插入失败。修复：取邮箱 `@` 前缀作为默认 name。
2. **Claude 数据源切换**：`sessions.db` 为空、`costs.jsonl` 全为 0。参考 [ccusage](https://github.com/ryoppippi/ccusage) 改用 `~/.claude/projects/**/*.jsonl` 作为数据源，成功读取真实 token 数据。
3. **后端时间格式不兼容**：客户端发送 RFC3339 带时区格式（`2026-05-30T10:06:40.971284+00:00`），后端 `TokenUsageReportReqVO.reportedAt` 使用 `LocalDateTime` 无法解析。修复：改为 `OffsetDateTime`，service 层转换。
4. **report-token-usage 命令 OOM**：命令被加到 async_mode daemon 初始化列表中，启动时内存占用过高被 OOM kill。修复：加到 daemon 跳过列表。
5. **二进制拷贝损坏**：多次拷贝过程中二进制损坏导致 SIGKILL。修复：先 rm 再 cp。

### 4.4 端到端测试

```bash
# 客户端自动从 Claude 本地数据读取并上报
easylife-ai report-token-usage claude-code
# 输出: [git-ai token-report] claude-code reported: 270771 tokens
```

**测试结果：** ✅ 端到端测试通过
- 成功读取 Claude 最新 session 数据（270771 tokens）
- 成功上报到后端并写入 `llm_token_usage` 表
- 数据正确包含 input/output/cache_read/cache_creation tokens
- Stop hook 在多轮对话中正常触发（UPSERT 更新同一条记录）
- `project_name` 字段正确提取并存储（如 `xm/demo`）

### 4.5 Stop hook 多轮对话验证

| Turn | 时间 | 后端日志行为 | Token 累计 |
|------|------|------------|-----------|
| 第1轮 | 10:27:23 | `Token 用量上报成功`（插入新记录） | 96,924 |
| 第2轮 | 10:28:19 | `Token 用量更新: old_total=96924, new_total=194927` | 194,927 |
| 第3轮 | 10:29:01 | `Token 用量更新: old_total=194927, new_total=270771` | 270,771 |

**结论：** 同一 session 只有一条记录，多次上报自动 UPSERT 累加更新。

### 4.6 Hook 配置

Claude Code 的 hook 配置有两个文件，`easylife-ai install-hooks` 会自动管理：

**`~/.claude/settings.json`**（所有用户都会写入）：
```json
{
  "hooks": {
    "PreToolUse": [...],
    "PostToolUse": [...],
    "Stop": [{
      "matcher": "*",
      "hooks": [{
        "type": "command",
        "command": "/Users/xz/.git-ai/bin/easylife-ai report-token-usage claude-code"
      }]
    }]
  }
}
```

**`~/.claude/hooks/hooks.json`**（ECC 插件用户才会写入）：
- 如果安装了 everything-claude-code（ECC）插件，Claude Code 优先读取此文件
- `install-hooks` 检测到该文件存在时，会自动写入 Stop hook
- 未安装 ECC 的用户不受影响，不会创建此文件

> **结论：** `easylife-ai install-hooks` 兼容有/无 ECC 插件两种情况，无需手动修改。

### 4.7 客户端安装配置

安装脚本支持通过环境变量配置 tracker：

```bash
TRACKER_URL="http://localhost:39527" \
TEAM_ID="2" \
TEAM_KEY="backend-2024-def456" \
USERNAME="xiaozhi" \
bash install-local.sh
```

安装后生成 `~/.git-ai/tracker-config.json`：
```json
{
  "tracker_url": "http://localhost:39527",
  "team_id": "2",
  "team_key": "backend-2024-def456",
  "username": "xiaozhi",
  "blacklist": []
}
```

> `USERNAME` 为可选参数。设置后上报使用该用户名；不设置则自动从 `git config user.email` 获取。

### 4.8 username 配置

上报时优先使用 `tracker-config.json` 中的 `username` 字段，未配置则自动从 `git config user.email` 获取。

**涉及文件：**

| 文件 | 改动 |
|------|------|
| `src/commands/tracker/config.rs` | `TrackerConfig` 新增 `username: Option<String>` |
| `src/commands/report_token_usage/mod.rs` | 优先读 config.username，回退到 git email |
| `install-local.sh` / `install.sh` | 支持 `USERNAME` 环境变量写入 tracker-config.json |
| `install-local.ps1` / `install.ps1` | 同上（Windows） |

## 5. 已知限制

1. ~~**Codex 只有总 token 数**~~：已修复（2026-06-01）。Codex 数据源已切换为 JSONL 会话日志（参照 ccusage），可获取 input/output/cache_read 明细。
2. ~~**Cursor 暂不支持**~~：已修复（2026-06-01）。Cursor Codex 与原生 Codex 共享同一 JSONL 数据源，cron 每分钟兜底上报。
3. **Stop hook 异常退出**：Claude/Codex 异常退出时 Stop hook 可能不触发 → cron 兜底可部分缓解
4. **数据源变更**：Claude Code 的 `sessions.db` 和 `costs.jsonl` 可能不包含真实 token 数据，改用 `~/.claude/projects/**/*.jsonl`（ccusage 方案）
5. **project_name 提取规则**：从 Claude 文件路径 `~/.claude/projects/-Users-<user>-<project>/` 提取，格式为 `<org>/<project>`，Codex 暂不支持自动提取
6. **mtime 不可靠**：Codex JSONL 文件的 mtime 可能在重新打开旧会话时被更新，排序改用文件名中的时间戳

---

## 6. 与现有 tracker 的关系

| 维度 | 现有 tracker（commit 上报） | Token 用量上报（新增） |
|------|---------------------------|---------------------|
| 触发时机 | `git push` | AI 会话结束（Stop hook + cron 兜底） |
| 数据粒度 | commit 级 | session 级 |
| 数据来源 | git diff + easylife-ai stats | 平台本地 JSONL 日志 |
| 上报内容 | 代码行数、AI 占比 | token 数、费用 |
| 数据库表 | `ai_stats_raw` | `llm_token_usage`（新） |
| 复用配置 | tracker-config.json | ✅ 复用 |
| 复用 HTTP 模块 | crate::http | ✅ 复用 |

---

## 7. 触发机制全覆盖（2026-06-01 更新）

### 7.1 三种触发路径

| 场景 | 触发方式 | 状态 |
|------|---------|------|
| 原生 Codex CLI | `~/.codex/hooks.json` Stop hook | ✅ |
| Cursor Codex | cron 每分钟检查 `~/.git-ai/check-codex-sessions.sh` | ✅ |
| Claude Code | `~/.claude/settings.json` Stop hook | ✅ |

### 7.2 Codex 上报架构

```
~/.codex/sessions/**/*.jsonl（所有 Codex 变体共享）
  ├── 原生 Codex CLI 写入 ✅
  ├── Cursor Codex 写入 ✅
  └── VSCode Codex 写入 ✅

触发方式:
  ├── Stop hook（~/.codex/hooks.json）→ 原生 CLI 即时触发
  └── cron * * * * * → 每分钟兜底，覆盖 Cursor + 旧会话增长
```

### 7.3 旧会话重新打开处理

Codex 重新打开旧会话时，会在**同一个 JSONL 文件**里追加新的 `token_count` 事件，累计总数增长：

```
Session 019e815e: 63610 → 77049 → 90817（同一文件，同一 session_id）
```

- codex.rs 按**文件名时间戳**排序（非 mtime，mtime 不可靠）
- 始终上报最新 session 的累计值
- 后端 UPSERT 比较 total，数据增长则更新，否则跳过
- 无需 marker 文件去重

### 7.4 Codex JSONL 数据特性

Codex 的 `input_tokens` **包含** `cached_input_tokens`，上报时需拆分：

| Codex JSONL 字段 | 上报字段 | 说明 |
|-----------------|---------|------|
| `input_tokens` | `input_tokens - cached_input_tokens` | 非缓存 input |
| `cached_input_tokens` | `cache_read_tokens` | 缓存读取 |
| `output_tokens` | `output_tokens` | 输出 |
| `total_tokens` | 后端计算 | `= (input-cached) + output + cached` |

验证：`(63313-55680) + 297 + 55680 = 63610` = Codex `total_tokens` ✅

### 7.5 Cursor hooks 限制

Cursor 的 hooks 系统（`~/.cursor/hooks.json`）与 Codex CLI（`~/.codex/hooks.json`）是**两个独立的系统**：

| | 原生 Codex CLI | Cursor Codex |
|---|---|---|
| Hooks 文件 | `~/.codex/hooks.json` | `~/.cursor/hooks.json` |
| 支持的事件 | PreToolUse / PostToolUse / **Stop** | afterFileEdit / beforeSubmitPrompt / postToolUse / preToolUse |
| Stop 事件 | ✅ 支持 | ❌ **不支持** |

因此 Cursor 里使用 Codex 时，Stop hook 不会触发，需要通过 cron 兜底上报。
