# 项目看板（project-kanban）

DeepSeek Harness Web UI 插件：左侧栏出现一个看板图标，点开是「项目看板」主面板，一屏查看在途项目的研发进展。

## 它显示什么

| 列 | 含义 |
|---|---|
| 规划中 | 立项/调研/提案阶段的项目 |
| 研发中 | 正在推进的项目与活跃代码库 |
| 验证评审 | 测试/验收/评审阶段 |
| 已交付 | 已完成/已上线 |
| 暂停归档 | 暂停、阻塞、归档 |

每张卡片来自真实数据，不是手填：

- **项目**：`~/Documents/aiworkspace/aiknowledge/projects/**/*.md` 里带 `status` 或 `type: project` frontmatter 的文档。阶段取自 `status`，进度优先取「关键里程碑」表格的 done/进行中/待办，其次取文档里的 `- [x]` / `- [ ]` 勾选比例；「最新进展」取文档中带日期的记录或「## 研发推进」小节。
- **目标**：`aiknowledge/goals/*.md`（如 221 产品市场化计划），进度来自计划勾选。
- **代码库**：`~/Documents/aiworkspace` 下每个含 `.git` 的仓库。显示最近提交、30 天提交数、未提交改动、分支、8 周提交柱状图；14 天无提交标为停滞，最后一条提交含 `archive/归档/暂停` 归入暂停。

停滞判定：研发中/验证评审超过 21 天没更新、规划中超过 30 天，卡片打黄色「停滞 N 天」标记。

## 数据流

```
Host 插件 index.js ── GET /kanban/board.json ──► Client 面板 fetch 渲染
        │  扫描 md frontmatter / 里程碑表格 / checklist / git
        └─ 20s 缓存，?refresh=1 强制刷新
```

面板每 60 秒自动刷新一次，右上角也可手动刷新（带强刷参数）。

## 配置

`config.json`（与 `index.js` 同目录，改完刷新面板即生效）：

```json
{
  "projectsRoot": "~/Documents/aiworkspace/aiknowledge",
  "reposRoot": "~/Documents/aiworkspace",
  "repoDeny": ["archive", "docs", "aiknowledge", "node_modules", "target"]
}
```

- `projectsRoot`：含 `projects/` 与 `goals/` 的知识库根目录。
- `reposRoot`：扫描一层子目录作为代码库的根目录。
- `repoDeny`：不当作代码库卡片的目录名。

## 开发自检

```bash
node --check plugins/project-kanban/index.js
node --check plugins/project-kanban/client.js
node plugins/project-kanban/dev/preview.mjs   # 打印真实扫描结果摘要
```

安装（由 Harness 的 plugin_manager 完成，写入当前 profile）：

```
plugin_manager(action: "install_bundle", target: "/绝对路径/plugins/project-kanban")
```

安装后刷新 Web 页面，左侧栏出现看板图标；页面内 `GET /kanban/board.json` 可直接查看原始数据。
