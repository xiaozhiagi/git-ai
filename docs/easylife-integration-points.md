# easylife-ai 集成点清单（fork ↔ upstream git-ai）

本文档记录 `easylife-ai` 分支在 upstream `git-ai`（trace2 异步架构）之上保留的全部定制点。
**目标：把 fork 改动收敛到少数有标记的位置，后续长期拉取 main 时代码冲突最小化。**

## 设计原则

1. **不改动 upstream 核心行为**：trace2 驱动、异步 daemon、thin git proxy 全部保持 main 原样。
2. **fork 代码独立成模块**：命令分发、prompt 存储、上报逻辑都在 `easylife_*` / `commands/tracker` / `commands/report_token_usage` 等 fork 自有文件里。
3. **集成点少量、有 `// easylife-ai:` 标记**：upstream 文件中只有下面列出的几处小改动。
4. **安装脚本隔离**：`install.sh` / `install.ps1` 采用 upstream 版本；fork 的网络安装脚本为 `install2.sh` / `install2.ps1`，离线安装脚本为 `install-local.sh` / `install-local.ps1`。

## 文件级集成点（upstream 文件中的 fork 标记改动）

| upstream 文件 | 改动 | 说明 |
| --- | --- | --- |
| `src/commands/git_ai_handlers.rs` | 1 个 guarded match arm + 1 个 daemon 豁免 match + help 调用 | `easylife_dispatch::is_easylife_command` 兜底分发；`backup`/`report-token-usage`/`ops-check`/`doctor`/`legacy-cleanup` 免 daemon；`uninstall` 保持 upstream 语义（`uninstall-hooks` 别名），fork 完整卸载器为 `easylife-uninstall`（避免与 upstream 别名冲突、不误杀共享 daemon） |
| `src/authorship/post_commit.rs` | `write_note` 后 1 个调用 | `persist_prompts_to_internal_db`：把 note 的 prompt 元数据镜像进内部 DB（异步旁路，非 trace2 ingestion 热路径） |
| `src/authorship/internal_db.rs` | 2 个 `pub(crate)` 连接访问器 | `fork_prompts_conn` / `fork_prompts_conn_mut`，供 `easylife_compat` 的 prompts 表 API 使用 |
| `src/commands/git_handlers.rs` | push 前后 2 个标记块 | tracker 推送上报的 pre-push ref 快照与成功后上报（见下方"已知偏差"） |
| `src/mdm/agents/{codex,cursor,vscode,windsurf,claude_code}.rs` | hook 内容差异 | Stop 钩子挂 `report-token-usage`（token 上报）而非 checkpoint；OpenCode 插件带 `session.idle` 上报 |
| `agent-support/opencode/git-ai.ts` | `session.idle` 上报 | fork 插件逻辑，plugin 导出保持单个 |
| `.github/workflows/release.yml` | `package-easylife-ai` job + install2 固定版本步骤 | 打包 `easylife-ai-*.zip` 与 `install2.sh/ps1` |
| `.gitignore` | 移除 `docs` 忽略 | upstream 跟踪 docs/，fork 文档（含本文件）需要入库 |
| `README.md` | 采用 upstream 版本 | fork 功能说明书移至 `docs/easylife-tracker-guide.md` |

## fork 自有模块（与 upstream 无文本冲突）

- `src/commands/easylife_dispatch.rs` — fork 子命令注册与分发（backup、uninstall、flush-cas、share、sync-prompts、prompts、prompt、search、continue、tracker、report-token-usage、ops-check、doctor、legacy-cleanup）。
- `src/commands/easylife_compat.rs` — prompt 历史兼容层：`PromptDbRecord` + `InternalDatabase` prompts 表 API（upstream 已删除、schema 仍在）、fork 版带内联消息的 `PromptRecord`、`find_prompt_with_db_fallback`、`redact_secrets_from_prompts`、`update_prompt_from_tool`、`persist_prompts_to_internal_db`。
- `src/commands/{backup,uninstall,flush_cas,prompts_db,prompt_replay,search,continue_session,share,share_tui,prompt_picker,sync_prompts,ops,doctor,legacy_cleanup,legacy_migration}.rs` — fork 命令实现。
- `src/commands/tracker/` — 代码统计上报（config/diff/filter/log/notes/push_state/retry/upload）。
- `src/commands/report_token_usage/` — token 用量上报（claude/codex/opencode + `opencode_storage.rs` 适配层）。
- `install2.sh` / `install2.ps1` / `install-local.sh` / `install-local.ps1` — fork 安装脚本。
- `docs/easylife-tracker-guide.md` — fork 功能说明书（原 README 内容）。

## 已知偏差（upstream 重构后的功能差异）

1. **prompt 历史不再内嵌完整消息**：upstream sessions-v2 把 transcript 移出 checkpoint/note（改走 CAS 流式上传）。fork 现在把 note 元数据镜像进内部 DB，消息体通过既有 CAS/本地缓存机制惰性解析；`prompts populate` 的内联消息路径已按新 note 格式移除。
2. **`sync-prompts` 刷新降级**：upstream 删除了各 agent 的 `transcript_and_model_from_*` 读取器，fork 暂未移植，`update_prompt_from_tool` 对所有工具返回 Unchanged（记录跳过、不损坏数据）。如需恢复，把对应读取器移植为 fork 自有模块即可。
3. **push 上报的时序**：AGENTS.md 要求一切 git 处理 trace2 化；tracker 推送上报需要 push 前的 ref 快照（pre-push state），upstream 的 `PushCompleted` 事件不携带 pre-push OID，因此 fork 在 git proxy 的 push 路径保留了一个独立于 upstream 流程的快照+上报块。这是有意保留的 fork 偏差，注意未来 upstream 若提供 pre-push ref 数据需迁移。
4. **版本号**：fork 保持 `1.0.0` 发布线（Cargo.toml），不随 upstream 1.7.x；release channel 与更新检查逻辑为 fork 自有（`update_check_interval_seconds` / `update_release_url` 配置项）。

## 合并 upstream 后的检查清单

1. `cargo build` —— `easylife_compat` / `easylife_dispatch` / `tracker` / `report_token_usage` 是否需要适配 upstream API。
2. `grep -rn "easylife-ai:" src/` —— 逐个核对标记集成点是否仍存在、是否被 upstream 改动包围。
3. `git-ai help` —— fork help 段落仍在输出末尾。
4. `git-ai doctor` —— 两条上报链路端到端自检。
5. `task test TEST_FILTER=tracker`、`TEST_FILTER=token`、`TEST_FILTER=easylife` 等定向测试。
