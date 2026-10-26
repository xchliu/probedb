window.__ModuleLoader__.load({
  id: '@local/project-kanban',
  factory(require) {
    const React = require('react');
    const h = React.createElement;

    const AUTO_REFRESH_MS = 60000;
    const DAY_MS = 86400000;
    const COLUMN_IDS = ['plan', 'dev', 'verify', 'deliver', 'paused'];
    const KIND_IDS = ['project', 'goal', 'repo'];

    const DICT_ZH = {
      panel: '项目看板',
      title: '项目看板',
      subtitle: '在途项目研发进展',
      refresh: '刷新',
      refreshing: '刷新中',
      retry: '重试',
      loading: '正在加载看板…',
      empty: '暂无在途项目',
      'empty.hint': 'Host 扫描没有发现带 status 的项目文档、进行中的目标或代码库。',
      'empty.column': '暂无卡片',
      'error.title': '看板加载失败',
      'error.refresh': '刷新失败，仍显示上次成功的数据',
      'stats.total': '共 {n} 个在途项目',
      'stats.updated': '更新于 {time}',
      'stats.updatedUnknown': '更新时间未知',
      'stats.stale': '{n} 张停滞',
      'warnings.title': '扫描告警',
      'warnings.count': '{n} 条',
      'col.plan': '规划',
      'col.dev': '研发',
      'col.verify': '验证',
      'col.deliver': '交付',
      'col.paused': '暂停',
      'col.other': '其他',
      'kind.project': '项目',
      'kind.goal': '目标',
      'kind.repo': '代码库',
      'kind.other': '条目',
      'meta.owner': '负责人 {name}',
      'meta.updated': '更新于 {time}',
      'meta.updatedUnknown': '更新时间未知',
      'meta.stale': '停滞 {n} 天',
      'meta.staleUnknown': '停滞',
      'progress.unquantified': '未量化',
      'progress.percent': '{n}%',
      'detail.latest': '最新进展',
      'detail.highlights': '进展要点',
      'detail.tags': '标签',
      'detail.repo': '代码库',
      'detail.owner': '负责人',
      'detail.updated': '更新日期',
      'detail.status': '原始状态',
      'card.expand': '展开详情',
      'card.collapse': '收起详情',
      'repo.branch': '分支',
      'repo.lastCommit': '最后提交',
      'repo.commits30': '30 天提交',
      'repo.dirty': '未提交',
      'repo.clean': '干净',
      'repo.weeks': '近 8 周提交',
      'repo.weekAria': '近 8 周共 {n} 次提交',
      'repo.none': '暂无仓库数据',
      'time.today': '今天',
      'time.yesterday': '昨天',
      'time.daysAgo': '{n} 天前',
      'time.monthsAgo': '{n} 个月前',
      'time.yearsAgo': '{n} 年前',
      'value.none': '—',
      'a11y.column': '{name}列，{n} 张卡片',
      localeTag: 'zh-CN',
    };

    const DICT_EN = {
      panel: 'Kanban',
      title: 'Project Kanban',
      subtitle: 'R&D progress across in-flight projects',
      refresh: 'Refresh',
      refreshing: 'Refreshing',
      retry: 'Retry',
      loading: 'Loading board…',
      empty: 'No in-flight items',
      'empty.hint': 'The Host scan found no project docs with status, active goals, or repositories.',
      'empty.column': 'No cards',
      'error.title': 'Could not load the board',
      'error.refresh': 'Refresh failed; showing the last successful data',
      'stats.total': '{n} in-flight items',
      'stats.updated': 'Updated {time}',
      'stats.updatedUnknown': 'Update time unknown',
      'stats.stale': '{n} stale',
      'warnings.title': 'Scan warnings',
      'warnings.count': '{n}',
      'col.plan': 'Plan',
      'col.dev': 'Development',
      'col.verify': 'Verify',
      'col.deliver': 'Deliver',
      'col.paused': 'Paused',
      'col.other': 'Other',
      'kind.project': 'Project',
      'kind.goal': 'Goal',
      'kind.repo': 'Repo',
      'kind.other': 'Item',
      'meta.owner': 'Owner {name}',
      'meta.updated': 'Updated {time}',
      'meta.updatedUnknown': 'Update time unknown',
      'meta.stale': 'Stalled {n}d',
      'meta.staleUnknown': 'Stalled',
      'progress.unquantified': 'Not quantified',
      'progress.percent': '{n}%',
      'detail.latest': 'Latest',
      'detail.highlights': 'Highlights',
      'detail.tags': 'Tags',
      'detail.repo': 'Repository',
      'detail.owner': 'Owner',
      'detail.updated': 'Updated',
      'detail.status': 'Raw status',
      'card.expand': 'Expand details',
      'card.collapse': 'Collapse details',
      'repo.branch': 'Branch',
      'repo.lastCommit': 'Last commit',
      'repo.commits30': 'Commits (30d)',
      'repo.dirty': 'Uncommitted',
      'repo.clean': 'Clean',
      'repo.weeks': 'Last 8 weeks',
      'repo.weekAria': '{n} commits in the last 8 weeks',
      'repo.none': 'No repository data',
      'time.today': 'today',
      'time.yesterday': 'yesterday',
      'time.daysAgo': '{n} days ago',
      'time.monthsAgo': '{n} months ago',
      'time.yearsAgo': '{n} years ago',
      'value.none': '—',
      'a11y.column': '{name} column, {n} cards',
      localeTag: 'en-US',
    };

    const STYLE_TEXT = `
.pkb-root {
  box-sizing: border-box;
  display: flex;
  flex-direction: column;
  gap: 12px;
  height: 100%;
  min-height: 0;
  padding: 20px 24px 16px;
  overflow: hidden;
  background: var(--dsw-alias-bg-base);
  color: var(--dsw-alias-label-primary);
  font-size: 13px;
  line-height: 1.5;
}
.pkb-header {
  display: flex;
  flex: 0 0 auto;
  flex-wrap: wrap;
  align-items: flex-start;
  justify-content: space-between;
  gap: 12px;
}
.pkb-heading {
  display: flex;
  flex-direction: column;
  gap: 4px;
  min-width: 0;
}
.pkb-title-row {
  display: flex;
  flex-wrap: wrap;
  align-items: center;
  gap: 8px;
}
.pkb-title {
  margin: 0;
  font-size: 18px;
  font-weight: 600;
  line-height: 1.3;
  letter-spacing: 0.01em;
}
.pkb-subtitle {
  margin: 0;
  color: var(--dsw-alias-label-secondary);
  font-size: 12px;
}
.pkb-stats {
  display: flex;
  flex-wrap: wrap;
  gap: 6px;
  margin-top: 2px;
}
.pkb-stat {
  display: inline-flex;
  align-items: center;
  gap: 4px;
  padding: 1px 8px;
  border: 1px solid var(--dsw-alias-border-l1);
  border-radius: 999px;
  background: var(--dsw-alias-bg-layer-1);
  color: var(--dsw-alias-label-secondary);
  font-size: 11px;
  white-space: nowrap;
}
.pkb-stat--stale {
  border-color: var(--dsw-alias-state-warn-primary);
  color: var(--dsw-alias-state-warn-primary);
}
.pkb-actions {
  display: flex;
  align-items: center;
  gap: 8px;
}
.pkb-btn {
  display: inline-flex;
  align-items: center;
  gap: 6px;
  height: 28px;
  padding: 0 12px;
  border: 1px solid var(--dsw-alias-border-l2);
  border-radius: 8px;
  background: var(--dsw-alias-bg-layer-1);
  color: var(--dsw-alias-label-primary);
  font-family: inherit;
  font-size: 12px;
  line-height: 1;
  cursor: pointer;
}
.pkb-btn:hover:not(:disabled) {
  background: var(--dsw-alias-bg-layer-2);
}
.pkb-btn:disabled {
  color: var(--dsw-alias-label-secondary);
  cursor: progress;
}
.pkb-btn:focus-visible {
  outline: 2px solid var(--dsw-alias-brand-primary);
  outline-offset: 1px;
}
.pkb-btn--primary {
  border-color: var(--dsw-alias-brand-primary);
  color: var(--dsw-alias-brand-primary);
}
.pkb-spinner {
  width: 11px;
  height: 11px;
  border: 2px solid var(--dsw-alias-border-l2);
  border-top-color: var(--dsw-alias-brand-primary);
  border-radius: 50%;
  animation: pkb-spin 0.8s linear infinite;
}
@keyframes pkb-spin {
  to { transform: rotate(360deg); }
}
@media (prefers-reduced-motion: reduce) {
  .pkb-spinner { animation: none; }
}
.pkb-banner {
  display: flex;
  flex: 0 0 auto;
  flex-wrap: wrap;
  align-items: baseline;
  gap: 4px 10px;
  padding: 8px 12px;
  border: 1px solid var(--dsw-alias-border-l1);
  border-radius: 8px;
  background: var(--dsw-alias-bg-layer-1);
  color: var(--dsw-alias-label-secondary);
  font-size: 12px;
}
.pkb-banner--warn {
  border-color: var(--dsw-alias-state-warn-primary);
  color: var(--dsw-alias-state-warn-primary);
}
.pkb-banner--error {
  border-color: var(--dsw-alias-state-error-primary);
  color: var(--dsw-alias-state-error-primary);
}
.pkb-banner__title {
  font-weight: 600;
}
.pkb-banner__detail {
  flex: 1 1 240px;
  font-size: 11px;
  overflow-wrap: anywhere;
}
.pkb-warnings {
  display: flex;
  flex: 1 1 320px;
  flex-direction: column;
  gap: 2px;
  max-height: 76px;
  margin: 0;
  padding-left: 16px;
  overflow-y: auto;
  font-size: 11px;
}
.pkb-board {
  display: flex;
  flex: 1 1 auto;
  align-items: stretch;
  gap: 12px;
  min-height: 200px;
  padding-bottom: 4px;
  overflow-x: auto;
  overflow-y: hidden;
}
.pkb-col {
  display: flex;
  flex: 1 1 200px;
  flex-direction: column;
  gap: 8px;
  min-width: 208px;
  min-height: 0;
  padding: 10px;
  border: 1px solid var(--dsw-alias-border-l1);
  border-radius: 12px;
  background: var(--dsw-alias-bg-layer-1);
}
.pkb-col__head {
  display: flex;
  flex: 0 0 auto;
  align-items: center;
  gap: 6px;
  padding: 0 2px;
}
.pkb-col__name {
  margin: 0;
  font-size: 12px;
  font-weight: 600;
  letter-spacing: 0.02em;
}
.pkb-col__count {
  margin-left: auto;
  color: var(--dsw-alias-label-secondary);
  font-size: 11px;
  font-variant-numeric: tabular-nums;
}
.pkb-col__list {
  display: flex;
  flex: 1 1 auto;
  flex-direction: column;
  gap: 8px;
  min-height: 0;
  overflow-x: hidden;
  overflow-y: auto;
}
.pkb-col__empty {
  padding: 6px 2px;
  color: var(--dsw-alias-label-secondary);
  font-size: 11px;
  opacity: 0.75;
}
.pkb-dot {
  flex: 0 0 auto;
  width: 8px;
  height: 8px;
  border-radius: 50%;
  background: var(--dsw-alias-state-idle-primary);
}
.pkb-dot--plan { background: var(--dsw-alias-state-idle-primary); }
.pkb-dot--dev { background: var(--dsw-alias-brand-primary); }
.pkb-dot--verify { background: var(--dsw-alias-state-warn-primary); }
.pkb-dot--deliver { background: var(--dsw-alias-state-success-primary); }
.pkb-dot--paused { background: var(--dsw-alias-state-error-primary); }
.pkb-dot--other { background: var(--dsw-alias-state-idle-primary); }
.pkb-card {
  position: relative;
  display: flex;
  flex-direction: column;
  gap: 6px;
  box-sizing: border-box;
  width: 100%;
  padding: 10px;
  border: 1px solid var(--dsw-alias-border-l1);
  border-radius: 10px;
  background: var(--dsw-alias-bg-base);
  color: var(--dsw-alias-label-primary);
  font-family: inherit;
  font-size: 12px;
  text-align: left;
  cursor: pointer;
  transition: border-color 0.15s ease, background 0.15s ease;
}
.pkb-card:hover {
  border-color: var(--dsw-alias-border-l2);
  background: var(--dsw-alias-bg-layer-2);
}
.pkb-card:focus-visible {
  outline: 2px solid var(--dsw-alias-brand-primary);
  outline-offset: 1px;
}
.pkb-card[aria-expanded="true"] {
  border-color: var(--dsw-alias-brand-primary);
  background: var(--dsw-alias-bg-layer-2);
}
.pkb-card__top {
  display: flex;
  align-items: center;
  gap: 6px;
  min-width: 0;
}
.pkb-card__spacer {
  flex: 1 1 auto;
}
.pkb-kind {
  display: inline-flex;
  align-items: center;
  height: 16px;
  padding: 0 6px;
  border: 1px solid var(--dsw-alias-border-l2);
  border-radius: 4px;
  color: var(--dsw-alias-label-secondary);
  font-size: 10px;
  font-weight: 600;
  letter-spacing: 0.03em;
}
.pkb-kind--project {
  border-color: var(--dsw-alias-brand-primary);
  color: var(--dsw-alias-brand-primary);
}
.pkb-kind--goal {
  border-color: var(--dsw-alias-state-warn-primary);
  color: var(--dsw-alias-state-warn-primary);
}
.pkb-kind--repo {
  border-color: var(--dsw-alias-state-success-primary);
  color: var(--dsw-alias-state-success-primary);
}
.pkb-caret {
  flex: 0 0 auto;
  color: var(--dsw-alias-label-secondary);
  font-size: 10px;
  line-height: 1;
}
.pkb-card__title {
  display: -webkit-box;
  overflow: hidden;
  margin: 0;
  font-size: 13px;
  font-weight: 600;
  line-height: 1.35;
  overflow-wrap: anywhere;
  -webkit-box-orient: vertical;
  -webkit-line-clamp: 2;
}
.pkb-card__sub {
  overflow: hidden;
  color: var(--dsw-alias-label-secondary);
  font-size: 11px;
  text-overflow: ellipsis;
  white-space: nowrap;
}
.pkb-progress {
  display: flex;
  flex-direction: column;
  gap: 3px;
}
.pkb-progress__track {
  height: 4px;
  overflow: hidden;
  border-radius: 999px;
  background: var(--dsw-alias-bg-layer-2);
}
.pkb-progress__fill {
  height: 100%;
  border-radius: 999px;
  background: var(--dsw-alias-brand-primary);
}
.pkb-progress__row {
  display: flex;
  align-items: baseline;
  justify-content: space-between;
  gap: 8px;
  color: var(--dsw-alias-label-secondary);
  font-size: 10px;
}
.pkb-progress__label {
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}
.pkb-progress__value {
  flex: 0 0 auto;
  font-variant-numeric: tabular-nums;
}
.pkb-progress__unknown {
  color: var(--dsw-alias-label-secondary);
  font-size: 10px;
}
.pkb-meta {
  display: flex;
  flex-wrap: wrap;
  align-items: center;
  gap: 2px 8px;
  color: var(--dsw-alias-label-secondary);
  font-size: 10px;
}
.pkb-meta__item {
  overflow: hidden;
  max-width: 100%;
  text-overflow: ellipsis;
  white-space: nowrap;
}
.pkb-meta__stale {
  color: var(--dsw-alias-state-warn-primary);
  font-weight: 600;
}
.pkb-snippet {
  display: -webkit-box;
  overflow: hidden;
  margin: 0;
  color: var(--dsw-alias-label-secondary);
  font-size: 11px;
  line-height: 1.45;
  overflow-wrap: anywhere;
  -webkit-box-orient: vertical;
  -webkit-line-clamp: 2;
}
.pkb-source {
  overflow: hidden;
  padding-top: 4px;
  border-top: 1px dashed var(--dsw-alias-border-l1);
  color: var(--dsw-alias-label-secondary);
  font-size: 10px;
  text-overflow: ellipsis;
  white-space: nowrap;
}
.pkb-detail {
  display: flex;
  flex-direction: column;
  gap: 8px;
  padding-top: 6px;
  border-top: 1px solid var(--dsw-alias-border-l1);
}
.pkb-detail__group {
  display: flex;
  flex-direction: column;
  gap: 3px;
}
.pkb-detail__label {
  color: var(--dsw-alias-label-secondary);
  font-size: 10px;
  font-weight: 600;
  letter-spacing: 0.04em;
  text-transform: uppercase;
}
.pkb-detail__text {
  margin: 0;
  color: var(--dsw-alias-label-primary);
  font-size: 11px;
  line-height: 1.5;
  overflow-wrap: anywhere;
}
.pkb-list {
  display: flex;
  flex-direction: column;
  gap: 3px;
  margin: 0;
  padding-left: 14px;
  font-size: 11px;
  line-height: 1.45;
  overflow-wrap: anywhere;
}
.pkb-tags {
  display: flex;
  flex-wrap: wrap;
  gap: 4px;
}
.pkb-tag {
  display: inline-flex;
  align-items: center;
  height: 16px;
  padding: 0 6px;
  border-radius: 4px;
  background: var(--dsw-alias-bg-layer-2);
  color: var(--dsw-alias-label-secondary);
  font-size: 10px;
}
.pkb-repo {
  display: flex;
  flex-direction: column;
  gap: 4px;
  padding: 6px 8px;
  border: 1px solid var(--dsw-alias-border-l1);
  border-radius: 8px;
  background: var(--dsw-alias-bg-base);
}
.pkb-repo__name {
  color: var(--dsw-alias-label-primary);
  font-size: 11px;
  font-weight: 600;
}
.pkb-repo__row {
  display: flex;
  align-items: baseline;
  justify-content: space-between;
  gap: 8px;
  color: var(--dsw-alias-label-secondary);
  font-size: 10px;
}
.pkb-repo__value {
  overflow: hidden;
  max-width: 70%;
  color: var(--dsw-alias-label-primary);
  font-variant-numeric: tabular-nums;
  text-align: right;
  text-overflow: ellipsis;
  white-space: nowrap;
}
.pkb-repo__branch {
  color: var(--dsw-alias-brand-primary);
}
.pkb-repo__subject {
  overflow: hidden;
  color: var(--dsw-alias-label-primary);
  font-size: 10px;
  text-overflow: ellipsis;
  white-space: nowrap;
}
.pkb-spark {
  display: flex;
  align-items: flex-end;
  gap: 2px;
  height: 26px;
  margin-top: 2px;
}
.pkb-spark__bar {
  flex: 1 1 0;
  min-width: 2px;
  min-height: 2px;
  border-radius: 2px 2px 0 0;
  background: var(--dsw-alias-brand-primary);
}
.pkb-spark__bar--zero {
  background: var(--dsw-alias-border-l2);
}
.pkb-state {
  display: flex;
  flex: 1 1 auto;
  flex-direction: column;
  align-items: center;
  justify-content: center;
  gap: 10px;
  min-height: 200px;
  padding: 32px 16px;
  border: 1px dashed var(--dsw-alias-border-l2);
  border-radius: 12px;
  background: var(--dsw-alias-bg-layer-1);
  color: var(--dsw-alias-label-secondary);
  text-align: center;
}
.pkb-state__title {
  color: var(--dsw-alias-label-primary);
  font-size: 13px;
  font-weight: 600;
}
.pkb-state__hint {
  max-width: 460px;
  font-size: 11px;
  line-height: 1.5;
  overflow-wrap: anywhere;
}
.pkb-sr-only {
  position: absolute;
  width: 1px;
  height: 1px;
  margin: -1px;
  padding: 0;
  overflow: hidden;
  border: 0;
  white-space: nowrap;
  clip: rect(0 0 0 0);
}
@media (max-width: 720px) {
  .pkb-root { padding: 14px 12px; }
  .pkb-col { min-width: 196px; }
  .pkb-title { font-size: 16px; }
}
`;

    function isRecord(value) {
      return value !== null && typeof value === 'object' && Array.isArray(value) === false;
    }

    function readText(value) {
      return typeof value === 'string' ? value.trim() : '';
    }

    function readNumber(value) {
      return typeof value === 'number' && Number.isFinite(value) ? value : null;
    }

    function readTextList(value) {
      const out = [];
      if (!Array.isArray(value)) return out;
      for (const entry of value) {
        const text = readText(entry);
        if (text !== '') out.push(text);
      }
      return out;
    }

    function readNumberList(value) {
      const out = [];
      if (!Array.isArray(value)) return out;
      for (const entry of value) {
        const num = readNumber(entry);
        out.push(num === null ? 0 : num);
      }
      return out;
    }

    function clamp01(value) {
      if (value < 0) return 0;
      if (value > 1) return 1;
      return value;
    }

    function readColumn(card) {
      const column = readText(card.column);
      return column === '' ? 'dev' : column;
    }

    function readKind(card) {
      const kind = readText(card.kind);
      return KIND_IDS.indexOf(kind) >= 0 ? kind : 'other';
    }

    function isKnownColumn(column) {
      return COLUMN_IDS.indexOf(column) >= 0;
    }

    function columnName(t, column) {
      return isKnownColumn(column) ? t('col.' + column) : t('col.other');
    }

    function cardKeyOf(card, column, index) {
      const id = readText(card.id);
      return id === '' ? column + ':' + String(index) : id;
    }

    function toDate(value) {
      const text = readText(value);
      if (text === '') return null;
      const iso = /^\d{4}-\d{2}-\d{2}$/.test(text) ? text + 'T00:00:00' : text;
      const date = new Date(iso);
      return Number.isNaN(date.getTime()) ? null : date;
    }

    function formatCount(value, localeTag) {
      try {
        return new Intl.NumberFormat(localeTag).format(value);
      } catch (failure) {
        return String(value);
      }
    }

    function formatStamp(value, localeTag) {
      const date = toDate(value);
      if (date === null) return null;
      try {
        return new Intl.DateTimeFormat(localeTag, {
          month: '2-digit',
          day: '2-digit',
          hour: '2-digit',
          minute: '2-digit',
          hour12: false,
        }).format(date);
      } catch (failure) {
        return readText(value);
      }
    }

    function formatDateOnly(value, localeTag) {
      const date = toDate(value);
      if (date === null) return null;
      try {
        return new Intl.DateTimeFormat(localeTag, {
          year: 'numeric',
          month: '2-digit',
          day: '2-digit',
        }).format(date);
      } catch (failure) {
        return readText(value);
      }
    }

    function relativeLabel(value, now, t) {
      const date = toDate(value);
      if (date === null) return null;
      const days = Math.floor((now - date.getTime()) / DAY_MS);
      if (days <= 0) return t('time.today');
      if (days === 1) return t('time.yesterday');
      if (days < 30) return t('time.daysAgo', { n: days });
      if (days < 365) return t('time.monthsAgo', { n: Math.floor(days / 30) });
      return t('time.yearsAgo', { n: Math.floor(days / 365) });
    }

    function groupCards(cards) {
      const groups = new Map();
      const order = [];
      for (const card of cards) {
        const column = readColumn(card);
        let bucket = groups.get(column);
        if (bucket === undefined) {
          bucket = [];
          groups.set(column, bucket);
          order.push(column);
        }
        bucket.push(card);
      }
      return { groups: groups, order: order };
    }

    function collectColumns(board, groups) {
      const plan = [];
      const seen = new Set();
      const declared = Array.isArray(board.columns) ? board.columns : [];
      for (const entry of declared) {
        if (!isRecord(entry)) continue;
        const id = readText(entry.id);
        if (id === '' || seen.has(id)) continue;
        seen.add(id);
        plan.push({ id: id, count: readNumber(entry.count) });
      }
      if (plan.length === 0) {
        for (const id of COLUMN_IDS) {
          if (seen.has(id)) continue;
          seen.add(id);
          plan.push({ id: id, count: null });
        }
      }
      for (const id of groups.order) {
        if (seen.has(id)) continue;
        seen.add(id);
        plan.push({ id: id, count: null });
      }
      return plan;
    }

    function readBoardResponse(response) {
      if (response === null || typeof response !== 'object') {
        return Promise.reject(new Error('invalid response'));
      }
      if (response.ok !== true) {
        return Promise.reject(new Error('HTTP ' + String(response.status)));
      }
      return response.json();
    }

    function requestBoard(force) {
      if (force === true) {
        return fetch('/kanban/board.json?refresh=' + String(Date.now()), { cache: 'no-store' });
      }
      return fetch('/kanban/board.json', { cache: 'no-store' });
    }

    function fetchBoard(force) {
      return Promise.resolve()
        .then(function () {
          return requestBoard(force);
        })
        .then(readBoardResponse)
        .then(function (payload) {
          if (!isRecord(payload)) throw new Error('invalid board payload');
          return payload;
        });
    }

    function Spinner() {
      return h('span', { className: 'pkb-spinner', 'aria-hidden': 'true' });
    }

    function StateBlock(props) {
      return h('div', { className: 'pkb-state' },
        props.spinner === true ? h(Spinner, null) : null,
        h('div', { className: 'pkb-state__title' }, props.title),
        typeof props.hint === 'string' && props.hint !== '' ? h('div', { className: 'pkb-state__hint' }, props.hint) : null,
        props.action === null || props.action === undefined ? null : props.action
      );
    }

    function KanbanIcon(props) {
      const size = props !== null && typeof props.size === 'number' && props.size > 0 ? props.size : 24;
      const active = props !== null && props.active === true;
      return h('svg', {
        viewBox: '0 0 24 24',
        width: size,
        height: size,
        'aria-hidden': 'true',
        focusable: 'false',
        style: { display: 'block', opacity: active ? 1 : 0.9 },
      },
        h('rect', {
          key: 'col-a',
          x: 3,
          y: 3.5,
          width: 5.2,
          height: 17,
          rx: 1.5,
          fill: 'none',
          stroke: 'currentColor',
          strokeWidth: 1.5,
        }),
        h('rect', {
          key: 'col-b',
          x: 9.4,
          y: 3.5,
          width: 5.2,
          height: 12,
          rx: 1.5,
          fill: 'none',
          stroke: 'currentColor',
          strokeWidth: 1.5,
        }),
        h('rect', {
          key: 'col-c',
          x: 15.8,
          y: 3.5,
          width: 5.2,
          height: 15,
          rx: 1.5,
          fill: 'none',
          stroke: 'currentColor',
          strokeWidth: 1.5,
        }),
        h('rect', {
          key: 'card-a',
          x: 4.3,
          y: 5.4,
          width: 2.6,
          height: 2.2,
          rx: 0.6,
          fill: 'currentColor',
          opacity: 0.9,
        }),
        h('rect', {
          key: 'card-b',
          x: 10.7,
          y: 5.4,
          width: 2.6,
          height: 2.2,
          rx: 0.6,
          fill: 'currentColor',
          opacity: 0.9,
        }),
        h('rect', {
          key: 'card-c',
          x: 17.1,
          y: 5.4,
          width: 2.6,
          height: 2.2,
          rx: 0.6,
          fill: 'currentColor',
          opacity: 0.9,
        })
      );
    }

    function ProgressBlock(props) {
      const t = props.t;
      const localeTag = props.localeTag;
      const value = readNumber(props.card.progress);
      const label = readText(props.card.progressLabel);
      if (value === null) {
        return h('div', { className: 'pkb-progress' },
          h('div', { className: 'pkb-progress__unknown' }, t('progress.unquantified')),
          label === '' ? null : h('div', { className: 'pkb-progress__label' }, label)
        );
      }
      const ratio = clamp01(value);
      const percent = Math.round(ratio * 100);
      return h('div', { className: 'pkb-progress' },
        h('div', { className: 'pkb-progress__track' },
          h('div', {
            className: 'pkb-progress__fill',
            style: { width: String(percent) + '%' },
          })
        ),
        h('div', { className: 'pkb-progress__row' },
          h('span', { className: 'pkb-progress__label' }, label),
          h('span', { className: 'pkb-progress__value' }, t('progress.percent', { n: formatCount(percent, localeTag) }))
        )
      );
    }

    function RepoSection(props) {
      const t = props.t;
      const localeTag = props.localeTag;
      const repo = isRecord(props.repo) ? props.repo : null;
      if (repo === null) {
        return h('div', { className: 'pkb-detail__text' }, t('repo.none'));
      }
      const name = readText(repo.name);
      const branch = readText(repo.branch);
      const lastCommitAt = formatDateOnly(repo.lastCommitAt, localeTag);
      const subject = readText(repo.lastCommitSubject);
      const commits30 = readNumber(repo.commits30);
      const dirty = readNumber(repo.dirty);
      const weeks = readNumberList(repo.weeks);
      let weekTotal = 0;
      let weekMax = 0;
      for (const count of weeks) {
        weekTotal += count;
        if (count > weekMax) weekMax = count;
      }
      const rows = [];
      rows.push(h('div', { className: 'pkb-repo__row', key: 'row-branch' },
        h('span', null, t('repo.branch')),
        h('span', { className: 'pkb-repo__value pkb-repo__branch' }, branch === '' ? t('value.none') : branch)
      ));
      rows.push(h('div', { className: 'pkb-repo__row', key: 'row-last-commit' },
        h('span', null, t('repo.lastCommit')),
        h('span', { className: 'pkb-repo__value' }, lastCommitAt === null ? t('value.none') : lastCommitAt)
      ));
      if (subject !== '') {
        rows.push(h('div', { className: 'pkb-repo__subject', key: 'row-subject', title: subject }, subject));
      }
      rows.push(h('div', { className: 'pkb-repo__row', key: 'row-commits30' },
        h('span', null, t('repo.commits30')),
        h('span', { className: 'pkb-repo__value' }, commits30 === null ? t('value.none') : formatCount(commits30, localeTag))
      ));
      rows.push(h('div', { className: 'pkb-repo__row', key: 'row-dirty' },
        h('span', null, t('repo.dirty')),
        h('span', { className: 'pkb-repo__value' },
          dirty === null ? t('value.none') : (dirty === 0 ? t('repo.clean') : formatCount(dirty, localeTag)))
      ));
      if (weeks.length > 0) {
        rows.push(h('div', { className: 'pkb-repo__row', key: 'row-weeks' },
          h('span', null, t('repo.weeks')),
          h('span', { className: 'pkb-repo__value' }, t('repo.weekAria', { n: formatCount(weekTotal, localeTag) }))
        ));
      }
      const bars = [];
      for (let index = 0; index < weeks.length; index += 1) {
        const count = weeks[index];
        const ratio = weekMax > 0 ? count / weekMax : 0;
        const height = count <= 0 ? 6 : Math.max(8, Math.round(ratio * 100));
        bars.push(h('div', {
          key: 'week-' + String(index),
          className: count <= 0 ? 'pkb-spark__bar pkb-spark__bar--zero' : 'pkb-spark__bar',
          style: { height: String(height) + '%' },
        }));
      }
      const chart = weeks.length === 0 ? null : h('div', { className: 'pkb-spark', 'aria-hidden': 'true' }, bars);
      return h('div', { className: 'pkb-repo' },
        name === '' ? null : h('div', { className: 'pkb-repo__name' }, name),
        rows,
        chart
      );
    }

    function CardView(props) {
      const t = props.t;
      const card = props.card;
      const now = props.now;
      const localeTag = props.localeTag;
      const expanded = props.expanded === true;
      const column = readColumn(card);
      const kind = readKind(card);
      const title = readText(card.title);
      const subtitle = readText(card.subtitle);
      const owner = readText(card.owner);
      const updated = readText(card.updated);
      const statusRaw = readText(card.statusRaw);
      const ageDays = readNumber(card.ageDays);
      const stale = card.stale === true;
      const latest = readText(card.latest);
      const highlights = readTextList(card.highlights);
      const tags = readTextList(card.tags);
      const source = readText(card.source);
      const repo = isRecord(card.repo) ? card.repo : null;
      const relative = relativeLabel(updated, now, t);
      const updatedLabel = relative === null ? t('meta.updatedUnknown') : t('meta.updated', { time: relative });
      const staleLabel = stale
        ? (ageDays === null ? t('meta.staleUnknown') : t('meta.stale', { n: ageDays }))
        : '';

      const top = h('div', { className: 'pkb-card__top' },
        h('span', { className: 'pkb-kind pkb-kind--' + kind }, t('kind.' + kind)),
        h('span', { className: 'pkb-card__spacer', 'aria-hidden': 'true' }),
        h('span', { className: 'pkb-dot pkb-dot--' + column, title: columnName(t, column), 'aria-hidden': 'true' }),
        h('span', { className: 'pkb-caret', 'aria-hidden': 'true' }, expanded ? '▾' : '▸')
      );

      const meta = h('div', { className: 'pkb-meta' },
        owner === '' ? null : h('span', { className: 'pkb-meta__item' }, t('meta.owner', { name: owner })),
        h('span', { className: 'pkb-meta__item' }, updatedLabel),
        staleLabel === '' ? null : h('span', { className: 'pkb-meta__stale' }, staleLabel)
      );

      const detailChildren = [];
      detailChildren.push(h('div', { className: 'pkb-detail__group', key: 'detail-facts' },
        h('div', { className: 'pkb-repo__row' },
          h('span', null, t('detail.owner')),
          h('span', { className: 'pkb-repo__value' }, owner === '' ? t('value.none') : owner)
        ),
        h('div', { className: 'pkb-repo__row' },
          h('span', null, t('detail.updated')),
          h('span', { className: 'pkb-repo__value' }, updated === '' ? t('value.none') : updated)
        ),
        h('div', { className: 'pkb-repo__row' },
          h('span', null, t('detail.status')),
          h('span', { className: 'pkb-repo__value' }, statusRaw === '' ? t('value.none') : statusRaw)
        ),
        h('div', { className: 'pkb-repo__row' },
          h('span', null, t('detail.repo')),
          h('span', { className: 'pkb-repo__value' }, repo === null ? t('value.none') : (readText(repo.name) === '' ? t('value.none') : readText(repo.name)))
        )
      ));
      detailChildren.push(h('div', { className: 'pkb-detail__group', key: 'detail-latest' },
        h('div', { className: 'pkb-detail__label' }, t('detail.latest')),
        h('p', { className: 'pkb-detail__text' }, latest === '' ? t('value.none') : latest)
      ));
      detailChildren.push(h('div', { className: 'pkb-detail__group', key: 'detail-highlights' },
        h('div', { className: 'pkb-detail__label' }, t('detail.highlights')),
        highlights.length === 0
          ? h('p', { className: 'pkb-detail__text' }, t('value.none'))
          : h('ul', { className: 'pkb-list' }, highlights.map(function (highlight, index) {
            return h('li', { key: 'highlight-' + String(index) }, highlight);
          }))
      ));
      if (tags.length > 0) {
        detailChildren.push(h('div', { className: 'pkb-detail__group', key: 'detail-tags' },
          h('div', { className: 'pkb-detail__label' }, t('detail.tags')),
          h('div', { className: 'pkb-tags' }, tags.map(function (tag, index) {
            return h('span', { className: 'pkb-tag', key: 'tag-' + String(index) }, tag);
          }))
        ));
      }
      if (repo !== null || kind === 'repo') {
        detailChildren.push(h('div', { className: 'pkb-detail__group', key: 'detail-repo' },
          h('div', { className: 'pkb-detail__label' }, t('detail.repo')),
          h(RepoSection, { repo: repo, t: t, localeTag: localeTag })
        ));
      }
      const detail = expanded ? h('div', { className: 'pkb-detail' }, detailChildren) : null;

      return h('div', {
        className: 'pkb-card',
        role: 'button',
        tabIndex: 0,
        'aria-expanded': expanded ? 'true' : 'false',
        onClick: props.onToggle,
        onKeyDown: function (event) {
          if (event.key === 'Enter' || event.key === ' ' || event.key === 'Spacebar') {
            event.preventDefault();
            props.onToggle();
          }
        },
      },
        top,
        h('div', { className: 'pkb-card__title' }, title === '' ? t('value.none') : title),
        subtitle === '' ? null : h('div', { className: 'pkb-card__sub', title: subtitle }, subtitle),
        h(ProgressBlock, { card: card, t: t, localeTag: localeTag }),
        meta,
        expanded === true ? null : (highlights.length === 0 ? null : h('p', { className: 'pkb-snippet' }, highlights[0])),
        detail,
        source === '' ? null : h('div', { className: 'pkb-source', title: source }, source),
        h('span', { className: 'pkb-sr-only' }, expanded ? t('card.collapse') : t('card.expand'))
      );
    }

    function ColumnView(props) {
      const t = props.t;
      const localeTag = props.localeTag;
      const column = props.column;
      const cards = props.cards;
      const count = column.count === null ? cards.length : column.count;
      const countText = formatCount(count, localeTag);
      const name = columnName(t, column.id);
      const cardEls = [];
      for (let index = 0; index < cards.length; index += 1) {
        const card = cards[index];
        const key = cardKeyOf(card, column.id, index);
        cardEls.push(h(CardView, {
          key: key,
          card: card,
          t: t,
          now: props.now,
          localeTag: localeTag,
          expanded: props.expanded[key] === true,
          onToggle: (function (target) {
            return function () {
              props.onToggle(target);
            };
          })(key),
        }));
      }
      return h('section', {
        className: 'pkb-col',
        'aria-label': t('a11y.column', { name: name, n: countText }),
      },
        h('div', { className: 'pkb-col__head' },
          h('span', {
            className: 'pkb-dot pkb-dot--' + (isKnownColumn(column.id) ? column.id : 'other'),
            'aria-hidden': 'true',
          }),
          h('h2', { className: 'pkb-col__name' }, name),
          h('span', { className: 'pkb-col__count' }, countText)
        ),
        h('div', { className: 'pkb-col__list' },
          cards.length === 0
            ? h('div', { className: 'pkb-col__empty' }, t('empty.column'))
            : cardEls
        )
      );
    }

    function BoardView(props) {
      const t = props.t;
      const localeTag = props.localeTag;
      const board = props.board;
      const hasBoard = board !== null;
      const now = props.now;
      const cards = hasBoard && Array.isArray(board.cards)
        ? board.cards.filter(function (card) {
          return isRecord(card);
        })
        : [];
      const groups = groupCards(cards);
      const columns = hasBoard ? collectColumns(board, groups) : [];
      const stats = hasBoard && isRecord(board.stats) ? board.stats : null;
      const declaredTotal = stats === null ? null : readNumber(stats.total);
      const total = declaredTotal === null ? cards.length : declaredTotal;
      let staleCount = 0;
      for (const card of cards) {
        if (card.stale === true) staleCount += 1;
      }
      const stamp = hasBoard ? formatStamp(board.generatedAt, localeTag) : null;
      const warnings = hasBoard ? readTextList(board.warnings) : [];

      const header = h('header', { className: 'pkb-header' },
        h('div', { className: 'pkb-heading' },
          h('div', { className: 'pkb-title-row' },
            h('h1', { className: 'pkb-title' }, t('title')),
            hasBoard && staleCount > 0
              ? h('span', { className: 'pkb-stat pkb-stat--stale' }, t('stats.stale', { n: formatCount(staleCount, localeTag) }))
              : null
          ),
          h('p', { className: 'pkb-subtitle' }, t('subtitle')),
          hasBoard
            ? h('div', { className: 'pkb-stats' },
              h('span', { className: 'pkb-stat' }, t('stats.total', { n: formatCount(total, localeTag) })),
              h('span', { className: 'pkb-stat' }, stamp === null ? t('stats.updatedUnknown') : t('stats.updated', { time: stamp }))
            )
            : null
        ),
        h('div', { className: 'pkb-actions' },
          h('button', {
            type: 'button',
            className: 'pkb-btn pkb-btn--primary',
            onClick: props.onRefresh,
            disabled: props.refreshing === true,
            'aria-label': t('refresh'),
            title: t('refresh'),
          },
            props.refreshing === true ? h(Spinner, null) : null,
            h('span', null, props.refreshing === true ? t('refreshing') : t('refresh'))
          )
        )
      );

      const warningsBlock = warnings.length === 0 ? null : h('section', {
        className: 'pkb-banner pkb-banner--warn',
        'aria-label': t('warnings.title'),
      },
        h('span', { className: 'pkb-banner__title' }, t('warnings.title')),
        h('span', null, t('warnings.count', { n: formatCount(warnings.length, localeTag) })),
        h('ul', { className: 'pkb-warnings' }, warnings.map(function (warning, index) {
          return h('li', { key: 'warning-' + String(index) }, warning);
        }))
      );

      const softError = hasBoard && props.error !== null
        ? h('div', { className: 'pkb-banner pkb-banner--error', role: 'status' },
          h('span', { className: 'pkb-banner__title' }, t('error.refresh')),
          h('span', { className: 'pkb-banner__detail' }, props.error),
          h('button', { type: 'button', className: 'pkb-btn', onClick: props.onRefresh }, t('retry'))
        )
        : null;

      let body = null;
      if (!hasBoard) {
        if (props.phase === 'error') {
          body = h(StateBlock, {
            spinner: false,
            title: t('error.title'),
            hint: props.error === null ? t('value.none') : props.error,
            action: h('button', { type: 'button', className: 'pkb-btn pkb-btn--primary', onClick: props.onRefresh }, t('retry')),
          });
        } else {
          body = h(StateBlock, {
            spinner: true,
            title: t('loading'),
            hint: null,
            action: null,
          });
        }
      } else if (cards.length === 0) {
        body = h(StateBlock, {
          spinner: false,
          title: t('empty'),
          hint: t('empty.hint'),
          action: null,
        });
      } else {
        body = h('div', { className: 'pkb-board' }, columns.map(function (column) {
          const bucket = groups.groups.get(column.id);
          return h(ColumnView, {
            key: 'column-' + column.id,
            column: column,
            cards: bucket === undefined ? [] : bucket,
            t: t,
            now: now,
            localeTag: localeTag,
            expanded: props.expanded,
            onToggle: props.onToggle,
          });
        }));
      }

      return h('section', {
        className: 'pkb-root',
        'aria-label': t('title'),
        'aria-busy': props.refreshing === true ? 'true' : 'false',
      },
        h('style', null, STYLE_TEXT),
        header,
        warningsBlock,
        softError,
        body
      );
    }

    function createBoardPage(t) {
      function BoardPage() {
        const [board, setBoard] = React.useState(null);
        const [phase, setPhase] = React.useState('loading');
        const [error, setError] = React.useState(null);
        const [refreshing, setRefreshing] = React.useState(false);
        const [now, setNow] = React.useState(function () {
          return Date.now();
        });
        const [expanded, setExpanded] = React.useState(function () {
          return {};
        });

        const boardRef = React.useRef(null);
        const ticketRef = React.useRef(0);
        const mountedRef = React.useRef(true);

        const load = React.useCallback(function (mode) {
          const force = mode === 'manual' || mode === 'retry';
          ticketRef.current += 1;
          const ticket = ticketRef.current;
          if (mode === 'initial' || mode === 'retry') {
            setPhase('loading');
            setError(null);
          } else if (mode === 'manual') {
            setRefreshing(true);
          }
          fetchBoard(force).then(function (payload) {
            if (mountedRef.current !== true || ticket !== ticketRef.current) return;
            boardRef.current = payload;
            setBoard(payload);
            setPhase('ready');
            setError(null);
            setRefreshing(false);
            setNow(Date.now());
          }).catch(function (failure) {
            if (mountedRef.current !== true || ticket !== ticketRef.current) return;
            const message = failure !== null && failure !== undefined && typeof failure.message === 'string' && failure.message !== ''
              ? failure.message
              : String(failure);
            setRefreshing(false);
            if (boardRef.current === null) {
              setPhase('error');
              setError(message);
            } else {
              setError(message);
              setNow(Date.now());
            }
          });
        }, []);

        React.useEffect(function () {
          mountedRef.current = true;
          load('initial');
          const timer = setInterval(function () {
            if (mountedRef.current !== true) return;
            setNow(Date.now());
            load('auto');
          }, AUTO_REFRESH_MS);
          return function () {
            mountedRef.current = false;
            ticketRef.current += 1;
            clearInterval(timer);
          };
        }, [load]);

        const onRefresh = React.useCallback(function () {
          load(boardRef.current === null ? 'retry' : 'manual');
        }, [load]);

        const onToggle = React.useCallback(function (key) {
          setExpanded(function (previous) {
            const next = {};
            for (const existing in previous) {
              if (Object.prototype.hasOwnProperty.call(previous, existing) && existing !== key) next[existing] = true;
            }
            if (previous[key] !== true) next[key] = true;
            return next;
          });
        }, []);

        return h(BoardView, {
          t: t,
          localeTag: t('localeTag'),
          board: board,
          phase: phase,
          error: error,
          refreshing: refreshing,
          now: now,
          expanded: expanded,
          onRefresh: onRefresh,
          onToggle: onToggle,
        });
      }
      return BoardPage;
    }

    return {
      inject: ['slots', 'locale'],
      apply(ctx) {
        ctx.effect(() => ctx.locale.register('projectKanban', {
          zh: DICT_ZH,
          en: DICT_EN,
        }), 'kanban: dictionaries');
        const t = ctx.locale.bind('projectKanban');
        const BoardPage = createBoardPage(t);
        ctx.slots.inject('main', () => ctx.slots.register({
          name: 'main',
          key: 'kanban',
          locale: 'projectKanban',
        }, BoardPage));
        ctx.slots.inject('sidebar.panellist', () => ctx.slots.register({
          name: 'sidebar.panellist',
          id: 'kanban',
          order: 20,
          locale: 'projectKanban',
          label: () => t('panel'),
        }, KanbanIcon));
      },
    };
  },
});
