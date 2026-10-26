# ProbeDB 作为 Hermes 记忆存储的候选评估

> 2026-10-26 · 基于 commit `d932a1a` + 本轮改动 · 结论是"小对象记忆候选成立，大存储不适用"

## 一、结论

| 数据类别 | 规模 | 是否候选 | 说明 |
|---|---:|---|---|
| `MEMORY.md` / `USER.md` | 当前 46 条 / 10.7 KB | ✅ **候选成立** | 建议先只读试点，再双写 |
| 会话历史（sessions） | 观察到的库为 0 字节，未使用 | ⚠️ 暂缓 | 需先明确实际写入路径与量级 |
| `state.db`（2.3 GB 级） | GB 级 | ❌ **不适用** | 全内存 + 全量快照模型，无法承载 |

**当前建议：不要替换 hermes-agent 内部存储。先用独立工具做只读试点。**

---

## 二、Hermes 当前记忆形态（实测）

- 路径：`~/.hermes/memories/MEMORY.md`、`~/.hermes/memories/USER.md`
- 格式：纯文本，条目之间用单独一行 `§` 分隔；条目前缀 `[标签]`
- 规模：MEMORY 30 条 + USER 16 条 = **46 条 / 10.7 KB**
- 现状痛点：整文件读写、无结构化检索、无并发控制、无校验和

任务不需要"重造 Hermes"，只需要让这 46 条记忆**可检索、可校验、可回滚**。

---

## 三、候选能力（已实现并验证）

```bash
python3 tools/probedb_mem.py import [文件...] [--append]   # 导入（默认重置）
python3 tools/probedb_mem.py search "关键词" [--json]       # 关键词检索（标签命中优先）
python3 tools/probedb_mem.py get <id> [--json]             # 按 id 读取
python3 tools/probedb_mem.py list [--limit N] [--json]      # 列出
python3 tools/probedb_mem.py add "内容" --tag T             # 追加
python3 tools/probedb_mem.py update <id> --content/--tag    # 更新
python3 tools/probedb_mem.py delete <id> --yes              # 删除
python3 tools/probedb_mem.py stats [--json]                 # 统计
```

- 每次写入自动 `persist()`：快照 + WAL，崩溃可恢复
- 多个 CLI 进程用 `fcntl` 文件锁串行化（实测 8 进程并发 `add`，8 条全部落盘）
- 字符串安全：`''` 转义、分号/括号/逗号/管道/反斜杠/换行均可入库
- 零外部依赖：Python 只用标准库，Rust 核心无 crate

---

## 四、实测数据

环境：macOS / Apple Silicon，release 构建，单一测试机。检索 = Python 侧全量取数 + 过滤（无索引）。

### 4.1 真实 Hermes 数据（46 条 / 10.7 KB）

| 指标 | 实测 |
|---|---:|
| 全量取数 | 0.48 ms |
| 关键词检索平均 | 0.18 ms |
| 重新打开（恢复） | 0.24 ms |
| CLI 单次调用（含 Python 启动） | 53.6 ms |

### 4.2 合成数据（每条约 80 汉字，标签池 8 个）

| 条目 | 导入 | DB 体积 | 重开 | 检索 avg | 检索 p95 | CLI 单次 |
|---:|---:|---:|---:|---:|---:|---:|
| 100 | 0.002 s | 22.3 KB | 0.24 ms | 0.32 ms | 0.33 ms | 54.8 ms |
| 1,000 | 0.015 s | 225.8 KB | 1.44 ms | 3.20 ms | 3.50 ms | 56.7 ms |
| 5,000 | 0.082 s | 1.1 MB | 6.96 ms | 16.5 ms | 17.8 ms | 78.8 ms |
| 10,000 | 0.150 s | 2.2 MB | 13.7 ms | 33.4 ms | 35.4 ms | 108.1 ms |

**解读**：
- 真实规模（46 条）下检索是亚毫秒级，CLI 成本 ~54 ms 几乎全是 Python 解释器启动；
- 即使 1 万条，CLI 也只需 ~108 ms，对"记忆检索"场景可接受；
- 导入性能不构成瓶颈（1 万条 0.15 s）。

复现：`python3 tools/bench_probedb.py --sizes 100,1000,5000,10000`

---

## 五、接入方案（推荐三步，可回退）

### Phase 0：只读试点（下一步）
- Hermes 在需要回忆时调用：`python3 tools/probedb_mem.py search "<query>" --json`
- **权威数据仍是 MEMORY.md/USER.md**，ProbeDB 只做索引副本，失败不影响原流程
- 观察命中率与延迟；此阶段零风险

### Phase 1：影子双写
- Hermes 写记忆时：先写原文件（成功为准），再调用 `add/update/delete` 同步 ProbeDB
- 同步失败只记日志，不回滚原流程
- 增加每日一致性检查（条数/内容 hash）

### Phase 2：切换
- ProbeDB 成为主存储；MEMORY.md 退化为"导出视图"（需要新增 `export` 命令）
- 保留原文件 + 导出能力作为回滚通道

**未完成的前置项**：`export`（ProbeDB → `§` 格式 md）、增量同步（避免 `import` 重置）、去重。

---

## 六、CLI 契约（Hermes 侧可直接依赖）

- 退出码：`0` 成功；`1` 业务失败（不存在/更新失败）；`2` 参数错误
- 错误走 stderr，正常结果走 stdout；`--json` 时 stdout 是**纯 JSON**（可直接 `json.loads`）
- JSON 字段：`id`(int)、`source`、`content`、`tags`、`created_at`(ISO 8601)
- 同一数据库同一时间只应有一个写进程；工具内部已加锁，直接调 FFI 的调用方需自行保证

示例：

```bash
$ python3 tools/probedb_mem.py search "坦哥思维" --json
[{"id": 4, "source": "MEMORY", "content": "[坦哥思维] 自然观察→哲学推演→实践落点。评价必须附具体事例。",
  "tags": "坦哥思维", "created_at": "2026-10-26T14:45:38"}]
```

---

## 七、风险与限制（为什么还不能替换内部存储）

1. **写入成本 O(n)**：`persist()` 全量重写快照。1 万条约 2.2 MB，可接受；10 万条预计 ~22 MB / 次写，需要增量快照后才能谈。
2. **无索引**：检索是全表 + Python 过滤，1 万条 33 ms；10 万条预计数百 ms。
3. **单写者模型**：文件锁只在工具内；Hermes 若多进程直连需额外协调。
4. **无事务**：多行导入中途失败时不会落盘（内存态丢弃），但也没有跨操作原子性。
5. **无类型化协议/参数绑定**：当前靠 `sql_literal()` 转义；直接写 SQL 的调用方需谨慎。
6. **无 `export`**：Phase 2 切换前必须补，否则回滚只能靠原始 md 备份。
7. **平台**：`fcntl` 文件锁适用于 macOS/Linux；Windows 需另做实现。

---

## 八、下一步

1. 坦哥确认是否允许动 `~/.hermes`；允许则做 Phase 0 只读试点，不允许则在仓库提供适配层 + 一条挂载命令。
2. 补 `export` 与增量同步，为 Phase 1 做准备。
3. 继续用真实数据观察检索质量（当前是子串匹配，不是语义检索；语义检索需要 embedding，属于后续）。
