# 项目看板（project-kanban）——实现契约

一个 DeepSeek Harness Web UI 插件：左侧栏图标 + 主面板「项目看板」，展示在途项目的研发进展。

- Host 半边（`index.js` + `host/scan.js`）：扫描真实数据源，注册只读 HTTP 路由 `/kanban/board.json`。
- Client 半边（`client.js`）：`main` 槽 key=`kanban` 的主面板 + `sidebar.panellist` 图标，`fetch` 上面的路由渲染看板。

## 文件所有权（不要越界写别人的文件）

| 文件 | 所有者 |
|---|---|
| `host/scan.js`, `dev/preview.mjs` | 数据扫描实现者 |
| `client.js` | 面板 UI 实现者 |
| `index.js`, `package.json`, `cordis.patch.yml`, `locale/*.json`, `icon.svg`, `CONTRACT.md` | Lead |

## 数据源（真实路径)

- `projectsRoot` = `~/Documents/aiworkspace/aiknowledge`
  - `projects/**/*.md`：项目文档。只收 frontmatter 有 `status` 或 `type: project` 的文件；跳过 `INDEX.md`。
  - `goals/*.md`：目标文档（跳过 `INDEX.md`）。只收 `status: active|进行中|active|dev*` 等未完成目标，`已完成/completed/done` 的目标归 `deliver`。
- `reposRoot` = `~/Documents/aiworkspace`：一层子目录里含 `.git` 的仓库，作为「代码库」卡片。

## Host 模块 API

`host/scan.js`（纯 ESM，只用 `node:` 内置模块，**不得** import 任何第三方包）：

```js
export async function buildBoard(options = {}) -> Promise<Board>
```

`options`：`{ projectsRoot?: string, reposRoot?: string, now?: Date, repoAllow?: string[], repoDeny?: string[], excludePaths?: string[] }`。
`excludePaths` 是相对 `projectsRoot` 的路径前缀，命中的项目文档不收录（默认 `["projects/企业AI体系建设"]`）。同 kind 下 title 完全相同的项目卡片要合并（优先保留不含 `/docs/`、路径更短的一条），合并数量写入 `warnings`。
默认值由 `index.js` 注入，`scan.js` 自己也要有 `os.homedir()` 兜底；路径允许以 `~/` 开头，用 `os.homedir()` 展开。
扫描失败不许抛：返回带 `warnings` 的 Board。

## Board JSON（渲染契约，必须完全一致）

```jsonc
{
  "generatedAt": "2026-10-26T08:12:00.000Z",   // ISO
  "warnings": ["可选：扫描告警"],
  "stats": { "total": 12, "byColumn": { "plan": 1, "dev": 6, "verify": 1, "deliver": 2, "paused": 2 } },
  "columns": [ { "id": "plan", "count": 1 }, ... ],   // 固定顺序 plan, dev, verify, deliver, paused
  "cards": [ Card, ... ]
}
```

```jsonc
Card = {
  "id": "projects/AI产品/meet-mionote-full-agent-mode.md",  // 稳定唯一 id（相对 projectsRoot 的路径；repo 用 "repo:<name>"）
  "kind": "project" | "goal" | "repo",
  "title": "Meet妙记 全Agent模式",
  "subtitle": "Meet妙记",              // 可选：产品名 / 类别
  "owner": "陈兴盛",                   // 可选
  "statusRaw": "developing",           // 原始状态字符串
  "column": "plan" | "dev" | "verify" | "deliver" | "paused",
  "progress": 0.62,                    // 0..1，未知为 null
  "progressLabel": "里程碑 5/8",       // 可选：进度来源说明
  "updated": "2026-09-17",             // ISO 日期（YYYY-MM-DD 即可），未知 null
  "ageDays": 39,                       // 整数，未知 null
  "stale": true,                       // ageDays 超阈值（dev/verify 21 天、plan 30 天）
  "latest": "2026-09-17：需求文档已转交陈兴盛…",  // 可选：最新一条带日期的记录
  "highlights": ["…最多 3 条短句…"],   // 研发进展/摘要要点，每条 ≤ 160 字
  "tags": ["project", "妙记"],         // 可选，最多 6 个
  "source": "projects/AI产品/meet-mionote-full-agent-mode.md", // 相对路径或仓库名，用于卡片脚注
  "repo": {                            // 仅 kind==="repo" 或该卡片关联代码库时出现
    "name": "probedb",
    "branch": "main",
    "lastCommitAt": "2026-10-26",
    "lastCommitSubject": "feat: SelfMind Phase 5.5 …",
    "commits30": 21,
    "dirty": 5,
    "weeks": [0,1,3,2,6,4,5,0]         // 最近 8 周每周提交数，旧→新
  }
}
```

排序：`cards` 按 `column` 顺序、再按 `updated` 降序（无日期最后）；`stale` 卡片靠后。client 端不再排序。

## 解析规则（Host 扫描实现）

1. **frontmatter**：文件首行 `---`，到下一个 `---` 之间的 `key: value`。支持中文键：`状态`、`负责人`、`更新日期`、`创建日期`、`产品`。
2. **status → column**（大小写不敏感、去空白/emoji）：
   - `plan`：proposal, plan, planning, proposed, idea, draft, research, 调研, 规划, 立项, 待启动
   - `dev`：active, developing, development, in_progress, in-progress, ongoing, wip, 进行中, 研发中, 推进中, 开发中
   - `verify`：review, reviewing, verifying, testing, acceptance, 验收, 评审, 测试, 灰度, 内测
   - `deliver`：done, completed, complete, delivered, shipped, released, launched, 已完成, 已交付, 已上线, 上线
   - `paused`：paused, blocked, stopped, on_hold, hold, archived, archive, dropped, 暂停, 归档, 阻塞, 搁置, 停止
   - 无法识别：`dev`
   - 无 frontmatter status 的 `type: project` 文档：按文件 mtime 判断，≤30 天 `dev`，否则 `paused`。
3. **progress（优先级从高到低）**：
   - 找到表头含 `状态` / `status` / `进度` 的 markdown 表格；统计其中 value 为 done/completed/完成/已完成 → 1；in_progress/进行中/推进中/⚠️ → 0.5；pending/planned/待定/未开始 → 0。progress = 加权和 / 行数；label = `里程碑 done/total`（total 为表格数据行数）。
   - 否则统计全文 `- [x]` / `- [ ]`（含 `* [x]`）：progress = done/total，label = `任务 done/total`。
   - 都没有：`progress: null`，`progressLabel` 省略。
4. **latest**：全文里匹配 `**YYYY-MM-DD**：文本` 或 `YYYY-MM-DD：文本`，取日期最大的一条；`highlights` 优先取 `## 研发推进|进展|进度|近况` 小节下的 bullet（去掉 markdown 修饰、去掉日期前缀），最多 3 条；没有该小节时取 `> 摘要/abstract` 引用或首个非空段落。截断到 160 字。
5. **repo**：`git -C <dir> log -1 --format=%ad|%s --date=short`、`git log --since="30 days ago" --oneline | wc`（用 `execFile` 拿 stdout 自己数）、`git status --porcelain` 行数、`git rev-parse --abbrev-ref HEAD`、`git log --since="8 weeks ago" --format=%ad --date=short` 按周聚合（ISO 周，旧→新）。
   - repo 的 column：最后提交 ≤14 天 → `dev`；15–90 天 → `paused`（并在 highlights 说明「N 天无提交」）；>90 天 → `paused`；最后一条提交标题含 `archive|归档|暂停` → `paused`。
   - repo 的 `progress: null`，`owner` 省略，`updated` = 最后提交日期。
6. **跳过**：`archive/`、`docs/` 等非项目目录下没有 frontmatter 的 md；`INDEX.md`；`.git`；隐藏目录；非 md 文件。
7. 全部文件读取、git 调用都要并行（`Promise.all`），整体预算 < 1.5s；单文件失败只记 warning。

## Client 渲染要求（client.js）

- 模块加载格式（window.__ModuleLoader__.load）：id 必须是 `@local/project-kanban`，`factory(require)` 里 `const React = require('react')`，**只允许** require `react`（禁止任何 `@deepseek-ai/*` 包）。
- 导出 `{ inject: ['slots', 'locale'], apply(ctx) {} }`。
- `apply` 内：
  - `ctx.effect(() => ctx.locale.register('projectKanban', { zh, en }), 'kanban: dictionaries')`
  - `const t = ctx.locale.bind('projectKanban')`
  - `ctx.slots.inject('main', () => ctx.slots.register({ name: 'main', key: 'kanban', locale: 'projectKanban' }, BoardPage))`
  - `ctx.slots.inject('sidebar.panellist', () => ctx.slots.register({ name: 'sidebar.panellist', id: 'kanban', order: 20, locale: 'projectKanban', label: () => t('panel') }, KanbanIcon))`
- `KanbanIcon` 接收 `{ size, active }`，画 24×24 SVG（三列卡片），`currentColor`，`aria-hidden`。
- `BoardPage`：
  - `fetch('/kanban/board.json', { cache: 'no-store' })`，加载中/失败可重试，每 60s 自动刷新，右上角「刷新」按钮。
  - 顶部：标题「项目看板」+ 副标题（在途项目研发进展）+ 统计（共 N 个在途项目、更新时间）。
  - 5 列横向看板，列头显示列名 + 计数，列内竖向卡片列表，横向滚动。
  - 卡片：标题、kind 徽标（项目/目标/代码库）、状态点、进度条（`progress` 为 null 显示「未量化」）、`progressLabel`、`owner`、`updated` 相对时间 + 停滞标记、`highlights` 第一条（2 行截断）、`source` 脚注；点击卡片展开/收起完整 highlights、latest、tags、repo 行（提交数/未提交/分支）。
  - repo 卡片额外显示 8 周提交量迷你柱状图（纯 div，高度按最大值归一）。
  - 只用主题 token 颜色：`--dsw-alias-bg-base/layer-1/layer-2/overlay`、`--dsw-alias-border-l1/l2`、`--dsw-alias-label-primary/secondary`、`--dsw-alias-brand-primary`、`--dsw-alias-state-success/warn/error/idle-primary`。禁止写死颜色。
  - 样式用 React `<style>` 元素（带唯一前缀 `.pkb-`），不要注入 document/head。
  - 键盘可达：按钮/卡片是 `<button>` 或带 `role="button"` + tabIndex。
  - 文案全部走 `t()`（zh + en 两份字典）。

## 校验

- 所有 JS：`node --check`（ESM：`node --input-type=module --check` 或临时 .mjs）。
- `dev/preview.mjs`：`node dev/preview.mjs` 打印真实扫描的 Board JSON 摘要（各列数量、前 3 张卡），不许写死假数据。
- 面板：不要求本地渲染（无浏览器控制），但要求 `node --check` 通过、只依赖 react、只读本契约字段。
