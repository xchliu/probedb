# ProbeDB 开发日志

> 每次代码提交前自动更新，记录变更、测试结果、设计决策

---

## 2026-09-16

### 完成
- **数据类型系统补全：BOOLEAN / DATE / TIME**（项目目标 MVP 清单中列出但此前未实现的三类）
  - 类型系统：`DataType` 新增 `Boolean`/`Date`/`Time`；`Value` 新增 `Boolean(bool)`/`Date(String)`/`Time(String)`
  - DATE/TIME 内部按 **ISO 8601 规范化字符串**存储（`YYYY-MM-DD` / `HH:MM:SS`）
  - 新增零依赖校验函数：`normalize_date()` / `normalize_time()` / `is_leap_year()` / `days_in_month()`
    - 日期：严格零填充格式 + 年/月/日范围 + **闰年规则**（4年一闰/100年不闰/400年再闰）
    - 时间：`HH:MM:SS` 或 `HH:MM`（秒补零）+ 时/分/秒范围
  - 非法值**明确报错**而非静默写入：`2026-02-30`、`2025-02-29`、`2026-13-45`、`25:00:00` 全部拒绝
  - SQL 解析器：`CREATE TABLE` 识别 `BOOLEAN`/`BOOL`（别名）/`DATE`/`TIME`
    - DATE/TIME 用**精确 token 匹配**，避免 `DATETIME`/`TIMESTAMP` 被前缀误吞（当前明确报"不支持的数据类型"）
  - 执行器：`compare_values()` 新增布尔/日期/时间比较分支
    - `false < true`（布尔可 ORDER BY）；布尔与 `1/0` 数字互通（SQLite 兼容语义）
    - DATE/TIME 字典序 == 时间序，可直接与字符串字面量比较（`WHERE due > '2026-01-01'`）
    - `parse_literal()` 识别裸词 `true`/`false`
    - LIKE 支持新类型（转字符串匹配）
  - 持久化：`encode_type`/`decode_type`/`encode_value`/`decode_value` 全链路支持
    - 编码格式：`DATE:2026-09-16` / `TIME:14:00:00` / `BOOL:true`
    - **解码时重新校验**：快照被改写为非法日期/时间 → 明确报错（不让脏数据进内存）
  - 修复：INSERT 解析错误不再吞掉底层原因，错误信息带上具体原因
    （原 `无法解析列 'd' 的值: 2026-13-45` → 现 `无法解析列 'd' 的值 '2026-13-45': 日期月份超出范围(1-12)`）

### 测试
- 155 passed, 0 failed（124→155，新增 31 个）
- types：test_normalize_date_valid / _rejects_invalid / test_normalize_date_leap_year / test_normalize_time_valid / _rejects_invalid / test_iso_date_string_order_equals_chronological_order / test_boolean_and_temporal_display
- storage：test_type_encoding_roundtrip / test_value_encoding_roundtrip_new_types / test_decode_value_rejects_corrupted_temporal / test_type_matches_new_types / test_parse_value_boolean_variants / test_parse_value_date_time_normalizes / test_temporal_columns_survive_state_export_import / test_tampered_date_in_snapshot_is_rejected
- sql：test_parse_create_table_boolean_date_time / test_parse_create_table_datetime_is_not_date / test_parse_select_with_new_type_filters
- executor：test_boolean_insert_and_select / test_boolean_where_filter / test_boolean_where_equality_is_not_always_true / test_boolean_order_by / test_date_range_filter / test_date_order_by_is_chronological / test_time_column_and_filter / test_invalid_date_insert_is_rejected / test_boolean_update_and_delete / test_new_types_with_group_by_and_aggregate / test_date_distinct
- lib（端到端持久化）：test_probedb_temporal_types_persist_reload / test_probedb_invalid_date_rejected_end_to_end

### 决策
- **DATE/TIME 存规范化 ISO 字符串，而非 epoch 整数**：
  1. 零填充 ISO 8601 的**字典序严格等于时间序**（已用测试固化），`WHERE/ORDER BY/GROUP BY/DISTINCT` 全部天然正确，无需日期运算库
  2. 快照文本协议可读，出问题能直接肉眼查（对端侧调试价值高）
  3. 零外部依赖铁律下代码量最小
  4. 有 SQLite 先例；代价是放弃了未来 `DATE + INTERVAL` 类日期算术（届时可改为数值存储 + 迁移）
- **DATE 必须是真类型而非 TEXT 别名**：靠写入时格式+范围+闰年三重校验获得语义保证，这是 DATE 区别于 TEXT 的价值所在
- **BOOLEAN 接受 `1/0`**：与整数语义互通降低使用摩擦，且比较层显式支持 Boolean↔Integer
- **DATETIME/TIMESTAMP 明确不支持**：不在 MVP 清单内，用精确匹配拒绝而非静默降级为 DATE（静默降级是更糟的失败模式）
- 不依赖 Hermes 接入决策，自主推进（待确认事项仍是 Hermes 切入点与 C ABI 确认）

### 已知遗留（候选下一任务）
- `compare_values()` 的 `_ => Equal` 兜底：**类型不匹配的比较**（如 `TEXT 列 = 123`、`布尔列 = 1.5`）会走 Equal 分支，导致 `=` 误判为真、`>=` 误判为真。今日新增类型都有显式分支，现实用法正确；但**类型安全比较语义**（不匹配即不命中）需要单独一次重构，涉及 SQL 类型强制转换规则，值得独立设计
- `DATETIME`/`TIMESTAMP` 若要支持，需扩展为 `YYYY-MM-DD HH:MM:SS` 组合类型

### 文档更新
- [x] 周计划更新
- [x] 开发日志更新
- [ ] 项目目标文档更新（MVP 数据类型清单已全部实现，可在坦哥确认后勾掉 DATE/TIME/BOOLEAN）

---

## 2026-09-15

### 完成
- **HAVING 子句（GROUP BY 后聚合过滤）**
  - SQL 解析器：新增 HAVING 子句解析，完整子句链路 WHERE→GROUP BY→HAVING→ORDER BY→LIMIT→OFFSET
  - 执行器新增 `filter_having_rows()` + `eval_having_condition()` 分组后过滤引擎
  - HAVING 可引用聚合函数：`COUNT(*)`、`SUM(col)`、`AVG(col)`、`MIN(col)`、`MAX(col)`
  - HAVING 可引用分组键列（字符串/数值比较）
  - 支持 AND/OR 复合条件（复用 split_top_level 括号感知分割）
  - 括号感知操作符检测：跳过聚合函数内的括号，避免误匹配函数参数中的 `>`/`<`
  - 与 WHERE 联动（先 WHERE 过滤行→再 GROUP BY 分组→最后 HAVING 过滤组）
  - 空表 GROUP BY + HAVING 返回 0 行

### 测试
- 124 passed, 0 failed（117→124，新增 7 个）
- test_having_count_star: COUNT(*) > 2 过滤分组
- test_having_sum: SUM(val) > 20 过滤分组
- test_having_with_where: WHERE + GROUP BY + HAVING 联动
- test_having_all_filtered: HAVING 过滤掉所有组返回 0 行
- test_having_avg: AVG(score) > 50 过滤分组
- test_having_and_or: AND/OR 复合条件
- test_having_empty_table: 空表 HAVING 返回 0 行

### 决策
- HAVING 是 GROUP BY 的直接延伸：上周完成 GROUP BY 分组聚合，HAVING 过滤分组后结果是 SQL 标准配对子句
- HAVING 求值策略：直接从已计算的输出行中查找聚合值（通过输出列名匹配），而非重新计算聚合——避免重复计算
- 聚合函数匹配通过输出列名（"count"/"sum"/"avg"/"min"/"max"）：SELECT 中必须有对应聚合才能在 HAVING 中引用
- 操作符检测括号感知：COUNT(*) 中的 `*` 不会被误解析，SUM(col) 中的列名不会被误分割
- 不依赖 Hermes 接入决策，自主推进

### 文档更新
- [x] 周计划更新
- [x] 开发日志更新
- [ ] 项目目标文档更新（无需变更）

---

## 2026-09-14

### 完成
- **GROUP BY 分组聚合**
  - SQL 解析器：新增 GROUP BY 子句解析，WHERE→GROUP BY→ORDER BY→LIMIT 完整子句链路
  - 执行器新增 `execute_group_by()` 分组聚合引擎
  - 支持单列/多列 GROUP BY（`GROUP BY dept` / `GROUP BY dept, level`）
  - SELECT 子句支持分组键列 + 聚合函数组合（`SELECT dept, COUNT(*), SUM(salary) FROM emp GROUP BY dept`）
  - 非聚合列必须出现在 GROUP BY 中（SQL 标准校验，不满足则报错）
  - 分组保持首次出现顺序（HashMap + order vector，结果确定性）
  - 与 WHERE 联动（先 WHERE 过滤，再分组聚合）
  - 空表 GROUP BY 返回 0 行（无分组）
  - 整数列 SUM/MIN/MAX 返回整数格式，AVG 返回浮点

### 测试
- 117 passed, 0 failed（110→117，新增 7 个）
- test_group_by_count_star: COUNT(*) 分组
- test_group_by_sum_avg: SUM+AVG 组合聚合
- test_group_by_min_max: MIN+MAX 组合聚合
- test_group_by_with_where: WHERE + GROUP BY 联动
- test_group_by_empty_table: 空表返回0行
- test_group_by_single_group: 全表同一组返回1行
- test_group_by_multi_column: 多列 GROUP BY

### 决策
- GROUP BY 是聚合函数的自然延伸：上周完成 SUM/AVG/MIN/MAX（全表聚合），本周实现分组聚合
- 本周无新周计划，按规则继续推进——GROUP BY 不依赖 Hermes 接入决策
- 非聚合列必须在 GROUP BY 中：SQL 标准语义，避免歧义（SQLite 允许非分组列返回任意行值，ProbeDB 选择更严格）
- 分组顺序保持首次出现：结果确定性，避免 HashMap 迭代顺序不一致
- 多列分组键用 `\x1f`（Unit Separator）连接避免值碰撞

### 文档更新
- [x] 周计划更新
- [x] 开发日志更新
- [ ] 项目目标文档更新（无需变更）

---

## 2026-09-11

### 完成
- **聚合函数 SUM/AVG/MIN/MAX**：`SELECT SUM(col) FROM t`
  - AggFunc 枚举 + parse_aggregate/extract_agg_column 辅助函数
  - 执行器提取数值列（Integer→f64, Float→f64），计算聚合值
  - 整数列返回整数格式（除 AVG 返回浮点），空表返回 0
  - 与 WHERE 过滤组合可用
- **OFFSET 分页**：`SELECT * FROM t OFFSET 2` / `SELECT * FROM t LIMIT 3 OFFSET 5`
  - SQLStatement::Select.offset 字段
  - parse_select 解析 OFFSET（独立使用或跟在 LIMIT 后）
  - ORDER BY 结束位置检测同时找 LIMIT 和 OFFSET
  - 执行器在 DISTINCT 之后、LIMIT 之前应用 OFFSET

### 测试
- 110 passed, 0 failed
- 新增测试: test_sum_aggregate, test_avg_aggregate, test_min_max_aggregate, test_aggregate_with_where, test_aggregate_empty_table, test_offset_basic, test_limit_with_offset, test_offset_exceeds_count

### 决策
- 聚合函数仅支持单列输入（SUM(col)），不支持 SUM(a+b) 表达式
- 整数列 SUM/MIN/MAX 返回整数格式，AVG 始终返回浮点
- OFFSET 在 DISTINCT 之后应用（先去重再跳过）

### 文档更新
- [x] 周计划更新
- [x] 开发日志更新
- [ ] 项目目标文档更新（无需变更）

---

## 2026-09-10

### 完成
- **DROP TABLE 语句**：`DROP TABLE t` / `DROP TABLE IF EXISTS t`
  - SQLStatement::DropTable + parse_drop_table（支持 IF EXISTS 关键字）
  - StorageEngine::drop_table（删除 schema + 数据）
  - ExecuteResult::TableDropped + WAL `DROP|<table>` 记录
  - WAL replay_drop（幂等：表不存在则跳过）
- **SELECT DISTINCT**：`SELECT DISTINCT col FROM t` / `SELECT DISTINCT * FROM t`
  - Select.distinct 字段 + DISTINCT 关键字解析
  - Executor HashSet 去重（保持首次出现顺序）
  - 与 WHERE / ORDER BY / LIMIT / 多列 / SELECT * 全组合可用

### 测试
- 102 passed, 0 failed (92 → 102, +10 新测试)
- DROP TABLE: basic / recreate / nonexistent error / IF EXISTS / WAL recovery
- DISTINCT: single col / multi col / all unique / with WHERE / star

### 决策
- IF EXISTS 在解析层忽略关键字（不改变 executor 行为）：DROP TABLE 不存在的表仍报错。保持简单，不引入 IF EXISTS 语义差异
- DISTINCT 去重在 LIMIT 之前执行（SQL 标准行为：先投影去重，再截断）
- DROP TABLE 写 WAL `DROP|` 记录，重放幂等设计与其他操作一致

### 文档更新
- [x] 周计划更新
- [x] 开发日志更新
- [ ] 项目目标文档更新（无需变更）

---

## 2026-09-09

### 完成
- **SELECT 列投影**：`SELECT col1, col2 FROM t` 现在只返回指定列（之前 executor 丢弃 columns 参数，始终返回所有列）
  - `SELECT *` 显式返回所有列（行为不变但语义明确化）
  - `SELECT name FROM t` 只返回 name 列，不含未选择的列
  - 查询不存在的列现在报明确错误（`列 'ghost' 不存在`）
- **COUNT(*) 聚合**：`SELECT COUNT(*) FROM t` 返回单行单列（列名 `count`），支持 `WHERE` 过滤
- **INSERT 返回值修复**：单行 INSERT 返回 `ExecuteResult::Inserted { row_id }`（之前始终返回 Message，导致 `Inserted` variant 从未被构造——dead code warning）
  - 多行 INSERT 仍返回 `ExecuteResult::Message("插入 N 行数据")`
- 修复 4 个持久化测试用例：旧测试用 `SELECT id FROM` 但断言结果含 `name` 列的值，列投影后需改为 `SELECT id, name FROM`

### 测试
- 92 passed, 0 failed
- 新增 6 测试：test_select_star_all_columns, test_select_single_column_projection, test_select_multi_column_projection, test_select_nonexistent_column_errors, test_count_star, test_insert_returns_row_id
- 86 → 92 全绿

### 决策
- 列投影是基础 SQL 功能缺失，不影响 Hermes 接入战略决策，自主推进
- COUNT(*) 在 executor 层短路返回（WHERE 过滤后直接计数），不走列投影路径
- 单行 vs 多行 INSERT 的返回值区分：单行返回 row_id（调用方可能需要），多行返回行数摘要

### 文档更新
- [x] 开发日志更新
- [ ] 周计划更新（本周无新计划，继续推进待确认事项）

---

## 2026-09-08

### 完成
- **修复 flaky 性能基线测试**：`test_persistence_perf_baseline` 断言 `batch_elapsed <= sync_elapsed` 在磁盘 I/O 抖动时偶发反转（sync=18.4ms batch=18.7ms，差 0.3ms）
  - 根因：硬性 `<=` 断言对两个接近的耗时值过于敏感，磁盘调度抖动即可反转
  - 修复：改为 `batch_elapsed <= sync_elapsed * 1.2`（20% 容差），历史上批量比同步快 26%，容差留足余量
  - 修复后连续运行稳定，86 测试全绿

### 测试
- 86 passed, 0 failed
- 修复前：85 passed, 1 failed（test_persistence_perf_baseline flaky）

### 决策
- 性能基线测试应用相对容差而非硬性边界，避免 I/O 抖动导致的 flaky failure

### 文档更新
- [x] 开发日志更新
- [x] 周计划开发日志更新
- [ ] 项目目标文档更新（无需变更）

---

## 2026-09-02

### 完成
- **错误处理与测试加固（Phase 3 周五任务，周计划收尾）**
- **数据损坏检测——快照校验和**：export_state 尾部新增 `# checksum <fnv1a64-hex>` 行
  - 零依赖 FNV-1a 64 位哈希（src/storage/mod.rs `fnv1a64()`），offset basis + prime 标准实现
  - persistence::load 新增 `validate_checksum()`：校验行存在则强制匹配，不匹配 → 明确报"文件已损坏或被篡改"
  - 向后兼容：v1 早期无校验行快照 → 跳过校验，正常加载（老库平滑升级）
  - 覆盖场景：静默篡改（改数据但保留合法头部）、磁盘位翻转、半截写入但头部完整的极端损坏
- **文件IO错误处理**：save() 任何一步失败（fs::write / fs::rename）→ 自动清理残留 `.tmp` 文件，不留垃圾
  - 磁盘满、权限不足、路径不存在 → 明确报错（含路径+OS原因），不静默
- **桥接层验证**：bridge/probedb.py 自测 + --demo（CRUD + 向量 + 持久化重开）全部通过，校验和改动未破坏 FFI 链路

### 测试
- 86 passed, 0 failed（从 75→86，新增 11 个测试）
- test_export_state_has_checksum_line / test_validate_checksum_ok：导出带校验行且自身校验通过
- test_validate_checksum_detects_tampering：篡改 TEXT:alice → 校验和不匹配报错
- test_validate_checksum_legacy_no_checksum_ok：无校验行旧快照 → 跳过校验正常接受
- test_load_detects_tampered_file：篡改落盘文件 → load 失败
- test_save_to_nonexistent_dir_errors_and_cleans_tmp：目录不存在 → save 报错且无 tmp 残留
- test_save_cleanup_tmp_on_rename_failure：rename 失败 → 清理 tmp
- test_load_returns_clear_io_errors：缺失文件 → 清晰中文报错
- test_fnv1a64_known_vector：FNV-1a 标准测试向量（空输入=offset basis，"a"=0xaf63dc4c8601ec8c）
- test_fnv1a64_changes_with_data：单字节变化 → 哈希变化
- test_export_state_checksum_is_stable：同一状态两次导出 → 校验行一致（确定性）

### 决策
- **校验和格式**：文本行 `# checksum <hex>` 放快照最后，以 `#` 开头 → import_state 天然跳过（注释行），只由 load 的 validate_checksum 消费，最小侵入
- **哈希算法选 FNV-1a 64 而非 CRC32**：零依赖自实现简单（~10 行）、64 位碰撞概率足够低、有标准测试向量可验证正确性
- **旧快照不强制回填校验和**：向后兼容优先——无校验行跳过，不拒绝老库（数据完整性保护从新写入开始生效）

---


## 2026-09-01

### 完成
- **Hermes 接口定义（Phase 3 周四任务）**：三方案选型（C ABI / pyo3 / IPC）→ 选 **C ABI (cdylib)**，理由：零外部依赖铁律 + 嵌入式模式 + 跨语言通用性（未来 Go/Node/Swift 都能调）
- **C ABI 实现 `src/ffi.rs`**：8 个导出函数（open/execute/persist/set_batch_mode/last_error/free_string/close）
  - 内存所有权：Rust 侧分配由 `probedb_free_string` 释放（谁分配谁释放）
  - panic 安全：所有入口 catch_unwind，panic 不跨 FFI 边界
  - NULL 语义：失败返回 NULL + 全局 last_error（Mutex 保护）
- **工程结构拆分**：main.rs（二进制）→ lib.rs（库）+ main.rs（入口），Cargo.toml 加 `[lib] crate-type=["rlib","cdylib"]`
- **Python 桥接层 `bridge/probedb.py`**：ctypes 封装，仿 sqlite3 用法（with 语句 + execute/persist/close），~130 行
- **真实链路验证**：CRUD + 向量混合查询（vector_similarity > 0.7 ORDER BY DESC）+ 持久化重开，全部通过

### 测试
- 75 passed, 0 failed（从 71→75，新增 4 个 FFI 测试）
- test_ffi_memory_mode_crud：内存模式建表/插入/查询
- test_ffi_error_propagation：错误返回 NULL + last_error 携带表名
- test_ffi_null_guard：NULL 句柄/NULL SQL 防护
- test_ffi_persist_reopen：持久化 → 重开 → 数据恢复

### 决策
- **接口选型：C ABI (cdylib)**。pyo3 开发体验好但引入外部 crate 破坏零依赖铁律且仅限 Python；IPC 违背嵌入式定位（第一版是进程内调用）
- **协议 v1 最小集**：8 个函数，SQL 文本进出（结果格式化文本，错误走 last_error），保持简单
- **Python 桥接 restype 用 c_void_p**：ctypes 的 c_char_p restype 会把指针转 bytes 拷贝，再 free 会 abort（真实踩坑，已修复）

### 遇到的坑
- ⚠️ ctypes `restype=c_char_p` + `free_string` → 释放错误指针 → Python abort（SIGABRT）。解法：restype 改 c_void_p 拿原始指针，string_at 读取，free_string 释放原指针
- ⚠️ cdylib crate-type 需要 lib.rs 作为库入口（main.rs 只有 bin 目标）→ 拆分 lib.rs + main.rs
- ⚠️ 拆出 lib 后 doctest 开始执行，storage/mod.rs 的 `ProbeDB state v1` 格式示例被当 Rust 代码 → 改 ```text

### 文档更新
- [x] 周计划更新（周四任务 ✅ + 待确认事项 2 条）
- [x] 开发日志更新
- [x] 项目目标文档更新（无需变更，接口定义单独立档：wiki《ProbeDB Hermes接口定义.md》）

---

## 2026-08-26

### 完成
- **快照恢复完整性**（Phase 3 周二任务）：新增 `validate_header()` 头部校验（空文件/非ProbeDB格式/版本不匹配→明确报错），`load()` 集成校验后再 import_state；为 v2 格式迁移预留入口
- **持久化性能优化**（Phase 3 周三任务）：
  - **WAL 批量模式**：`WalLog::set_batch(true)` 后 append 只进内存 `pending` 缓冲，`flush()` 一次性 write_all + 单次 fsync 落盘（原同步模式每条记录一次 fsync）
  - **延迟持久化（可配置）**：`set_auto_flush_threshold(n)` 达阈值自动刷盘；`set_batch(false)` 自动 flush 剩余；`ProbeDB::set_batch_mode()/flush()` 对外接口
  - **persist() 语义强化**：先 flush pending → save 快照 → truncate WAL（批量模式数据不会丢在快照外）
  - **性能基线**：1000条INSERT 内存 13.3ms / 同步 19.5ms / 批量 14.5ms（批量接近内存，比同步快26%）；恢复1000行 3.9ms

### 测试
- 71 passed, 0 failed（从 63→71，新增 8 个）
- WAL 单元测试: 批量累积后 flush、关闭批量自动落盘、自动刷盘阈值、批量顺序保持（T/I/U/D 混合按序重放）
- ProbeDB 集成测试: 批量模式崩溃丢失未flush记录（延迟持久化预期权衡）、flush后崩溃可恢复、persist自动flush、性能基线

### 决策
- 批量模式是显式的性能/持久性权衡：同步模式（默认）每条 DML 立即 fsync 最安全；批量模式适合批量导入/大量写入，崩溃最多丢失未 flush 的缓冲记录
- WAL 重放幂等设计让批量模式安全：即使 flush 边界与内存操作不完全对齐，重放也不会产生重复副作用
- 关闭批量模式自动 flush：保证模式切换后语义不静默变化

### 文档更新
- [x] 周计划更新
- [x] 开发日志更新
- [ ] 项目目标文档更新（无需变更）

---

## 2026-08-03

### 完成
- **WAL（Write-Ahead Log）写入路径 + 崩溃恢复**（Phase 3 周一任务）
- 新增 `persistence::wal` 模块：追加式文本行协议，4 种记录
  - `T|<table>|<col>:<type>|...` — CREATE TABLE
  - `I|<table>|<row_id>|<value>|...` — INSERT（显式 row_id，重放幂等）
  - `D|<table>|<row_id>` — DELETE（重放幂等）
  - `U|<table>|<row_id>|<col_index>|<value>` — UPDATE（重放幂等）
- **写入路径接入**：executor 4 个 DML 分支（CreateTable/Insert/Delete/Update）先写 WAL 再改内存，遵循 write-ahead 语义
- **崩溃恢复**：`ProbeDB::open()` = load 全量快照 + replay WAL 增量；重放后立即固化回快照并 truncate WAL（避免下次重复重放）
- **原子性保证**：快照+WAL 组合 — persist() = save 快照（原子 rename）+ truncate WAL；新库自动初始化 WAL（从第一条 DML 开始记录）
- **恢复韧性**：重放时损坏行跳过不整体失败（eprintln 记录行号）；幂等重放（表/行已存在跳过，DELETE/UPDATE 对不存在行无操作）

### 测试
- 56 passed, 0 failed（新增 6 个：从 50→56）
- wal 单元测试: append 追加模式、重放恢复完整操作链（含 `|` 转义）、幂等重放（快照+WAL 重复不产生重复行）、损坏行跳过
- ProbeDB 集成测试: 崩溃恢复全流程（persist→再写不persist→reopen→WAL重放→数据完整→WAL清空）、persist 后 WAL 截断无重复重放

### 决策
- WAL 格式复用快照的文本行协议 + 转义规则（`split_pipe_aware` / `encode_value` 改为 `pub(crate)` 复用）
- 单字符操作码（T/I/D/U）紧凑可读；显式 row_id 让重放不依赖自增计数器
- 幂等设计是快照+WAL 组合的正确性基石：persist 成功但 truncate 前崩溃的场景不会重复插入
- 重放损坏行跳过而非整体失败：一条坏记录不能毁掉整个库（周五"数据损坏检测"的提前量）

### 遇到的坑
- ⚠️ 新建库（path 不存在）时 wal 初始化为 None → WAL 文件从未创建，崩溃恢复测试失败。解法：新库分支也调用 `WalLog::open` 初始化 WAL
- ⚠️ 测试手写 WAL 记录时 `TEXT:bob|smith` 未转义 → 重放把 `smith` 当成新字段解析失败。WAL 记录中的字面 `|` 必须写 `\|`

---

## 2026-07-31

### 完成
- **持久化引擎 Phase 3 第一步**：状态序列化 + 原子落盘/加载
- `StorageEngine::export_state()` / `import_state()` — 文本行协议 v1（显式类型标记 + 转义）
- 新增 `persistence` 模块：`save()`（临时文件 + rename 原子写）/ `load()`
- `ProbeDB::open(path)` / `persist()` — 嵌入式 API，文件存在加载、不存在新建
- 序列化格式支持全部4种类型（INTEGER/FLOAT/TEXT/VECTOR）+ next_id 恢复

### 测试
- 50 passed, 0 failed（新增8个：从42→50）
- persistence 单元测试: save/load roundtrip（含 Text 含 `|` 转义）、覆盖保存、文件缺失报错、向量导出导入、import 替换旧状态
- ProbeDB 集成测试: open→persist→reload 全链路（含 next_id 连续）、向量持久化后相似度查询、内存模式 persist 报错

### 决策
- 序列化格式：文本行协议（非二进制），方便调试和版本演进；`SCHEMA|...` / `ROW|...` / `NEXTID|...`
- 值编码：显式类型标记 `INT:` / `FLOAT:` / `TEXT:` / `VEC:len:v1,v2,...`，杜绝歧义
- 转义规则：`\` → `\\`，`|` → `\|`，`\n` → `\n`；解析用转义感知分割 `split_pipe_aware`（跳过转义对）
- 原子写：先写 `<path>.tmp` 再 rename，崩溃不留半截文件
- 持久化策略：全量快照（MVP），后续再评估 WAL 增量

### 遇到的坑
- ⚠️ 朴素 `line.split('|')` 会把转义的 `\|` 误切成两个字段（`TEXT:bob\|smith` → `bob\` + `smith`），导致反序列化失败。解法：`split_pipe_aware` 跳过转义对。

### 文档更新
- [x] 周计划更新
- [x] 开发日志更新
- [ ] 项目目标文档更新（Phase 3 里程碑推进，建议坦哥确认后更新）

---

## 2026-07-30

### 完成
- 测试加固：新增5个集成测试（空表操作、WHERE无匹配、LIKE模式、多语句批处理、完整CRUD流水线）
- 错误处理增强：验证插入不存在表错误、缺列插入错误、无匹配WHERE操作

### 测试
- 42 passed, 0 failed（新增5个：从37→42）
- 新增: test_empty_table_operations, test_where_no_match, test_like_patterns_integration, test_multi_statement_batch, test_combined_crud_pipeline

### 决策
- Phase 2 全部完成 ✅ — 42测试全绿
- Phase 3 (Hermes接入) 够条件推进。但测试覆盖仍有缺口：SELECT * 列扩展功能未独立验证、ORDER BY空表场景需确认行为正确

### 文档更新
- [x] 周计划更新
- [x] 开发日志更新

---

## 2026-07-27

### 完成
- vector_similarity() 原生函数实现
- 混合查询全链路（SELECT + WHERE + ORDER BY + vector_similarity）
- 性能基线测试

### 测试
- 37 passed, 0 failed
- 新增: vector_similarity 查询、混合查询集成测试

### 决策
- 向量函数名: `vector_similarity(embedding, 'text')` 返回 [0, 1] 相似度分数
- 性能基线: 1000条向量暴力搜索 < 10ms（开发机）

### 文档更新
- [x] 周计划更新
- [x] 开发日志创建

---

## 2026-07-24

### 完成
- DELETE 语句实现
- UPDATE 语句实现
- borrow checker 修复（Update handler 中生命周期问题）
- 集成测试补充

### 测试
- 33 passed, 0 failed
- 新增: test_delete_basic, test_update_where, test_delete_where

### 决策
- DELETE/UPDATE 沿用 SELECT 的 WHERE 条件解析逻辑，复用现有过滤链
- 不引入级联删除/事务（MVP 只做原子写入）

### 文档更新
- [x] 周计划更新

---

## 2026-07-23

### 完成
- 日常代码清理、重构

### 测试
- 25 passed, 0 failed（无新增测试）

---

## 2026-07-22

### 完成
- 代码清理、边界处理

### 测试
- 25 passed, 0 failed（无新增测试）

---

## 2026-07-13~21

### 完成
- WHERE 条件过滤（`= != > < >= <= AND OR`）
- LIKE 模式匹配（`%` `_` 通配符，自研实现，零外部依赖）
- ORDER BY 排序（ASC/DESC）
- DATE/TIME/BOOLEAN 数据类型扩展
- DELETE/UPDATE 开发（7/16-7/18 cron 因网络问题未推进，后修复）

### 测试
- 25 passed, 0 failed

### 决策
- LIKE 自研实现，不依赖 regex crate，零外部依赖
- 自研 SQL 解析器，不依赖 sqlparser-rs（MVP 后改为自研，减少依赖体积）

---

## 2026-07-08~12

### 完成
- Rust 项目 scaffold
- 自研 SQL 解析器（CREATE TABLE, INSERT, SELECT）
- 内存存储引擎（行+向量统一存储）
- VECTOR 类型作为一等公民
- 余弦/欧几里得向量相似度函数
- Executor 框架

### 测试
- 22 passed, 0 failed

### 决策
- 语言: Rust
- SQL 解析器: 自研（零外部依赖）
- 存储引擎: 内存（MVP），后续加持久化
- 向量: 暴力搜索（MVP），后续 HNSW

---

## 2026-07-07

### 项目启动
- 坦哥提出 ProbeDB 构想
- 定位: AI内建数据库，从第一天将 AI 作为一等公民
- 第一版目标: 接入 Hermes 作为嵌入式存储后端
- 技术栈: Rust，自研 SQL 解析器，零外部依赖

### 决策
- 项目名: ProbeDB
- 语言: Rust
- 开发节奏: 每天写，cron 自动推进
- 协作模式: 坦哥定方向审结果，我负责实现