# Token 用量上报功能 - 实施文档

> 更新日期: 2026-06-15 14:15
> 状态: ✅ Phase 1+2+3 完成 | 端到端测试通过 | UPSERT 去重 | project_name | username 配置 | Codex JSONL 数据源 | Cron 兜底 | 服务端自动计费 | **用户输入 prompts 上报**

---

## 1. 已完成的工作

### 1.1 客户端（git-ai，Rust）

项目路径（更新文档时不允许删除，要保留）：/Users/xz/xm/srch-001-ai-tracker/git-ai

项目文档（更新文档时不允许删除，要保留）：
- 原开源项目git-ai原始文档：/Users/xz/xm/srch-001-ai-tracker/git-ai/README-old.md
- 基于开源项目做功能后的文档：/Users/xz/xm/srch-001-ai-tracker/git-ai/README.md

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

**Claude Code:** 在 `install_hooks_at` 中，除了现有的 PreToolUse/PostToolUse checkpoint 命令外，在 Stop hook 中也安装：
```json
{
  "hooks": {
    "PreToolUse": [...],
    "PostToolUse": [...],
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
Stop hook 同时写入 `settings.json`（所有用户）和 `hooks.json`（仅 ECC 插件用户），确保全覆盖。

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
6. **settings.json 缺少 Stop hook**：`install_hooks_at` 只写入 PreToolUse/PostToolUse，Stop hook 的 report-token-usage 命令仅写入 `hooks.json`（ECC 插件文件），非 ECC 用户无法自动上报 token 用量。修复（2026-06-15）：在 `install_hooks_at` 中增加 Stop hook 写入 `settings.json` 的逻辑，同时更新 `hook_status` 检测 Stop hook 是否存在，确保所有用户都能自动上报。

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

> **重要修复（2026-06-15）：** 之前 Stop hook 的 report-token-usage 命令仅写入 `hooks.json`（ECC 插件文件），非 ECC 用户无法自动上报。现已修复，`install_hooks_at` 会同时将 Stop hook 写入 `settings.json`，确保所有用户都能在会话结束时自动上报 token 用量。

**`~/.claude/hooks/hooks.json`**（ECC 插件用户额外写入）：
- 如果安装了 everything-claude-code（ECC）插件，Claude Code 优先读取此文件
- `install-hooks` 检测到该文件存在时，也会自动写入 Stop hook
- 此文件中的 Stop hook 是冗余保险，与 `settings.json` 中的 Stop hook 功能相同
- 未安装 ECC 的用户不受影响，不会创建此文件

> **结论：** `easylife-ai install-hooks` 兼容有/无 ECC 插件两种情况，所有用户都能自动上报 token 用量，无需手动修改。

### 4.6.5 卸载旧版本（安装前推荐）

**为什么需要卸载旧版本？**
- 避免多个版本冲突
- 清理旧的配置文件
- 确保新版本的环境变量优先级正确
- 避免旧版本的 hooks 干扰新版本

**卸载后会丢失什么？**
- `~/.git-ai/tracker-config.json` — tracker 配置（重新安装时需要重新配置）
- `~/.git-ai/config.json` — git-ai 基础配置
- hooks 配置（如果完全删除 Claude/Codex 目录）

#### macOS/Linux 卸载步骤

```bash
# 1. 停止后台进程（如果在运行）
pkill -f easylife-ai 2>/dev/null || true

# 2. 删除安装目录
rm -rf ~/.git-ai

# 3. 删除 PATH 环境变量配置（各个 shell）
# Bash
sed -i.backup '/easylife-ai/d' ~/.bashrc 2>/dev/null || true
sed -i.backup '/easylife-ai/d' ~/.bash_profile 2>/dev/null || true

# Zsh
sed -i.backup '/easylife-ai/d' ~/.zshrc 2>/dev/null || true

# Fish
sed -i.backup '/easylife-ai/d' ~/.config/fish/config.fish 2>/dev/null || true

# 4. 删除符号链接
rm -f ~/.local/bin/easylife-ai 2>/dev/null || true

# 5. 卸载 hooks（可选，如果需要保留 hooks 配置可跳过）
# Claude Code hooks
# 如果需要完全清理，可以手动编辑 ~/.claude/settings.json 删除相关配置
# 或者完全删除（谨慎！会丢失所有 Claude 配置）
# rm -rf ~/.claude

# Codex hooks
# 手动编辑 ~/.codex/hooks.json 删除 easylife-ai 相关配置
# 或者完全删除（谨慎！会丢失所有 Codex 配置）
# rm -rf ~/.codex

echo "✓ 卸载完成"
echo "请重启终端和 IDE 以生效"
```

#### Windows 卸载步骤

```powershell
# 1. 停止后台进程（如果在运行）
Get-Process | Where-Object {$_.ProcessName -like '*easylife-ai*'} | Stop-Process -Force -ErrorAction SilentlyContinue

# 2. 删除安装目录
Remove-Item -Recurse -Force "$env:USERPROFILE\.git-ai" -ErrorAction SilentlyContinue

# 3. 删除 PATH 环境变量配置
# 需要手动从系统环境变量中删除 %USERPROFILE%\.git-ai\bin

# 4. 卸载 hooks（可选，如果需要保留 hooks 配置可跳过）
# 手动编辑相关配置文件删除 easylife-ai 相关配置

Write-Host "✓ 卸载完成"
Write-Host "请重启终端和 IDE 以生效"
```

#### 建议的卸载策略

1. **完全重装**：执行上述所有步骤，包括删除 hooks
2. **保留 hooks**：跳过第 5 步，只删除二进制和配置
3. **备份配置**：卸载前先备份 `~/.git-ai/tracker-config.json`，重装后恢复

### 4.7 客户端安装配置（重新安装前请先卸载）

> **重要提示**：如果您已经安装过旧版本的 easylife-ai，强烈建议先执行 [4.6.5 节的卸载步骤](#465-卸载旧版本安装前推荐) 再进行安装，以避免版本冲突和配置问题。

安装脚本支持通过环境变量配置 tracker，并正确处理 USERNAME 环境变量。

#### 远程安装

从 GitHub releases 直接下载并安装：

```bash
curl -sSL https://github.com/xiaozhiagi/easylife-ai-666/releases/latest/download/install-easylife-ai.sh | \
  TRACKER_URL="http://192.168.110.146:39527" \
  TEAM_ID="2" \
  TEAM_KEY="backend-2024-def456" \
  USER_NAME="DN7374" \
  bash
```

#### 本地安装

```bash
TRACKER_URL="http://localhost:39527" \
TEAM_ID="2" \
TEAM_KEY="backend-2024-def456" \
USER_NAME="DN7374" \
bash install-local.sh
```

> **注意**：使用 `USER_NAME` 而非 `USERNAME`，因为 `USERNAME` 是 shell 内置变量，会被当前登录用户名覆盖。

#### 生成的配置文件

安装后生成 `~/.git-ai/tracker-config.json`：

```json
{
  "tracker_url": "http://localhost:39527",
  "team_id": "2",
  "team_key": "backend-2024-def456",
  "username": "DN7374",
  "blacklist": []
}
```

#### USERNAME 环境变量处理（修复于 2026-06-08）

**问题**：之前 USERNAME 环境变量传入后，生成的 `tracker-config.json` 中 username 却变成了系统 `git config user.email` 的值。

**修复方案**：`install-easylife-ai.sh` 脚本现在正确处理 USERNAME 环境变量，遵循以下优先级：

1. **优先使用 `USERNAME` 环境变量**（如果设置）
2. **回退到 `git config user.email`**（如果 USERNAME 未设置）
3. **显示警告**（如果两者都没有设置）

**安装时的行为**：

```bash
# 情况1：设置了 USERNAME，会显示
Configuring tracker with username: DN7374

# 情况2：未设置 USERNAME，自动使用 git email
Configuring tracker with username: xz (from git config user.email)

# 情况3：都没有设置，显示警告
⚠ Warning: Neither USERNAME env var nor git config user.email is set
```

**涉及文件**：
- `install-easylife-ai.sh` —— 修复于 2026-06-08，正确优先级处理
- `install-easylife-ai.ps1` —— Windows 版本（相同逻辑）

> **提示**：`USERNAME` 为可选参数。设置后上报使用该用户名；不设置则自动从 `git config user.email` 获取。安装时会显示实际使用的 username 值，便于验证配置是否正确。

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

---

## 8. Phase 3：用户输入 Query 上报（已完成）

> 目标：在 token 用量上报时，同时记录用户在该 session 中的真实输入内容，方便追溯"谁问了什么问题、花了多少 token"。
>
> 状态：✅ 客户端 + 后端 + 前端已完成，端到端验证通过（2026-06-15）

### 8.1 最终上报格式

同个 session 可能有多轮用户输入，按以下格式追加到 `user_prompts` 字段：

```
------------2026-06-09T14:30:00Z------------
帮我写一个 Python 日志解析器

------------2026-06-09T14:32:00Z------------
很好，已经能跑了，继续开发下一个功能

------------2026-06-10T12:35:00Z------------
添加单元测试
```

- 时间戳：ISO 8601 格式，直接从 JSONL 的 `timestamp` 字段提取
- 整体截断上限：**8000 字符**，超出时尾部追加 `...(truncated)`
- 截断策略：保留前面的轮次，丢掉后面的轮次

### 8.2 数据源分析

**关键发现**：`role: "user"` 的消息**不全是用户真实输入**，包含大量系统/工具注入的噪音。

#### Claude Code JSONL 消息分类

| 消息特征 | 是否真实用户输入 | 示例 |
|---------|---------------|------|
| `content` 是 **STRING** | ✅ 是 | `"修改更新文档路径..."` |
| `content` 是 **LIST**，item[0].type=`"tool_result"` | ❌ 否，工具结果 | `"[Fact-Forcing Gate]\nQuote the user's..."` |
| `content` 是 STRING，以 `"This session is being continued"` 开头 | ❌ 否，上下文续传 | `"This session is being continued from a previous conversation..."` |

**区分信号**：
- `permissionMode` 顶层 key 存在 → 通常是真实用户输入
- `toolUseResult` 顶层 key 存在 → 工具结果
- 但最可靠的方式是 **`content` 类型判断**：STRING = 用户输入，LIST = 工具结果

#### Codex JSONL 消息分类

| `event_msg.payload.type` | 是否真实用户输入 | 示例 |
|--------------------------|---------------|------|
| `"user_message"`，内容不以 `# Context from my IDE setup` 开头 | ✅ 是 | `"检查和测试token计数项目，生成4500行代码"` |
| `"user_message"`，内容以 `# Context from my IDE setup` 开头 | ❌ 否，IDE 自动注入 | `"# Context from my IDE setup:\n## Open tabs:..."` |
| 重复内容 | ❌ 否，IDE 每次打开编辑器重复注入 | 同上 |

### 8.3 客户端实现（Rust）

#### 新增文件/函数

| 文件 | 改动 |
|------|------|
| `src/commands/report_token_usage/claude.rs` | 新增 `extract_user_queries()` 函数 |
| `src/commands/report_token_usage/codex.rs` | 新增 `extract_user_queries()` 函数 |
| `src/commands/report_token_usage/mod.rs` | payload 新增 `user_prompts: Option<String>` 字段 |

#### Claude Code 提取逻辑

```rust
fn extract_user_queries(lines: &[Value], max_len: usize) -> Option<String> {
    let mut entries: Vec<(String, String)> = Vec::new();
    let mut seen = HashSet::new();

    for line in lines {
        let msg = line.get("message")?;
        if msg.get("role")?.as_str()? != "user" { continue; }

        // 关键过滤：content 必须是 STRING（排除 tool_result）
        let content = msg.get("content")?.as_str()?;

        // 过滤系统续传消息
        if content.starts_with("This session is being continued") { continue; }

        // 去重
        if seen.contains(content) { continue; }
        seen.insert(content.to_string());

        let ts = line.get("timestamp")?.as_str()?;
        entries.push((ts.to_string(), content.to_string()));
    }

    if entries.is_empty() { return None; }
    build_query_string(&entries, max_len)
}
```

#### Codex 提取逻辑

```rust
fn extract_user_queries(events: &[Value], max_len: usize) -> Option<String> {
    let mut entries: Vec<(String, String)> = Vec::new();
    let mut seen = HashSet::new();

    for line in events {
        if line.get("type")?.as_str()? != "event_msg" { continue; }
        let payload = line.get("payload")?;
        if payload.get("type")?.as_str()? != "user_message" { continue; }

        let content = payload.get("message")?.as_str()?;

        // 过滤 IDE 自动注入的上下文
        if content.starts_with("# Context from my IDE setup") { continue; }

        // 去重
        if seen.contains(content) { continue; }
        seen.insert(content.to_string());

        let ts = line.get("timestamp")?.as_str()?;
        entries.push((ts.to_string(), content.to_string()));
    }

    if entries.is_empty() { return None; }
    build_query_string(&entries, max_len)
}
```

#### 公共拼接函数

```rust
fn build_query_string(entries: &[(String, String)], max_len: usize) -> Option<String> {
    let mut result = String::new();
    for (i, (ts, content)) in entries.iter().enumerate() {
        if i > 0 { result.push('\n'); }
        result.push_str("------------");
        result.push_str(ts);
        result.push_str("------------\n");
        result.push_str(content);
    }
    if result.len() > max_len {
        result.truncate(max_len);
        result.push_str("\n...(truncated)");
    }
    Some(result)
}
```

### 8.4 后端实现（Java）

#### 数据库变更

```sql
ALTER TABLE llm_token_usage ADD COLUMN user_prompts TEXT;
```

#### 变更文件清单

| 文件 | 改动 |
|------|------|
| `sql/init.sql` | `llm_token_usage` 表新增 `user_prompts TEXT` 列 |
| `entity/LlmTokenUsage.java` | 新增 `userQuery` 字段 |
| `controller/vo/TokenUsageReportReqVO.java` | 新增 `userQuery` 字段（可选） |
| `service/impl/TokenUsageReportServiceImpl.java` | UPSERT 逻辑调整 |

#### UPSERT 语义调整

同一 session 多次上报时，`user_prompts` 覆盖规则：
- **新数据长度 >= 旧数据长度时才覆盖**
- 因为 session 进行中 query 会不断增加新轮次，保证最终存的是最完整版本
- 如果新数据中 `user_prompts` 为 null 或空，不覆盖已有值

### 8.5 上报 Payload 变更

新增字段后，完整 payload 如下：

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
  "user_prompts": "------------2026-06-09T14:30:00Z------------\n帮我写一个 Python 日志解析器\n\n------------2026-06-09T14:32:00Z------------\n很好，继续开发",
  "reported_at": "2026-05-29T16:00:00Z"
}
```

### 8.6 风险与注意事项

| 风险 | 应对措施 |
|------|---------|
| 提取失败 | 优雅降级，`user_prompts` 为 null，不影响 token 上报 |
| 超长 query | 整体截断 8000 字符 |
| 多轮 query 含敏感信息（代码、密码） | 后端可考虑脱敏处理，前端展示加权限控制 |
| Claude 子 agent 的 query | 子 agent 非用户直接对话，当前只读主 session JSONL，不读 `subagents/` 子目录 |
| 数据库存量数据兼容 | 新增列默认为 NULL，老记录不受影响 |
| JSONL 格式随版本变化 | 提取逻辑加容错，解析失败不影响主流程 |
