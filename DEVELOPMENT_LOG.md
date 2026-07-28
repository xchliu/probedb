# ProbeDB 开发日志

> 每次代码提交前自动更新，记录变更、测试结果、设计决策

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