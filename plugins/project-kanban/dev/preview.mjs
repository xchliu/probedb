#!/usr/bin/env node
/**
 * project-kanban · Host 扫描预览（开发用）
 *
 * 直接调用真实的 buildBoard()，打印 Board JSON 摘要：
 *   总卡片数、各列数量、每列前 3 张卡的标题/进度/更新时间、警告、耗时，
 *   并附完整单卡 JSON 便于核对 CONTRACT.md 的 Board/Card 结构。
 *
 * 用法：node plugins/project-kanban/dev/preview.mjs
 * 退出码：契约自检通过为 0，失败为 1。
 */

import { buildBoard } from '../host/scan.js'

const COLUMNS = ['plan', 'dev', 'verify', 'deliver', 'paused']
const COLUMN_LABEL = {
  plan: '规划',
  dev: '研发',
  verify: '验证',
  deliver: '交付',
  paused: '暂停',
}

const DATE_RE = /^\d{4}-\d{2}-\d{2}$/

function percent(progress) {
  return typeof progress === 'number' ? `${Math.round(progress * 100)}%` : '未量化'
}

function progressText(card) {
  if (typeof card.progress !== 'number') return '未量化'
  return `${percent(card.progress)}${card.progressLabel ? `（${card.progressLabel}）` : ''}`
}

function updatedText(card) {
  if (!card.updated) return '无更新日期'
  return card.updated + (typeof card.ageDays === 'number' ? `（${card.ageDays} 天前）` : '')
}

function lineOf(card) {
  const stale = card.stale ? ' [停滞]' : ''
  return `    ${card.title} — 进度 ${progressText(card)} · 更新 ${updatedText(card)} · kind=${card.kind}${stale}`
}

function validateBoard(board) {
  const issues = []
  const check = (condition, message) => {
    if (!condition) issues.push(message)
  }
  const isString = (value) => typeof value === 'string' && value.length > 0
  const isInt = (value) => Number.isInteger(value) && value >= 0

  check(board !== null && typeof board === 'object', 'board 不是对象')
  check(isString(board?.generatedAt) && !Number.isNaN(Date.parse(board.generatedAt)), 'generatedAt 不是合法 ISO')
  check(Array.isArray(board?.warnings) && board.warnings.every((item) => typeof item === 'string'), 'warnings 不是 string[]')
  check(Array.isArray(board?.cards), 'cards 不是数组')

  const cards = Array.isArray(board?.cards) ? board.cards : []
  const columns = Array.isArray(board?.columns) ? board.columns : []
  check(columns.length === COLUMNS.length, `columns 数量应为 ${COLUMNS.length}，实际 ${columns.length}`)
  COLUMNS.forEach((id, index) => {
    check(columns[index]?.id === id, `columns[${index}].id 应为 ${id}，实际 ${columns[index]?.id}`)
  })

  const byColumn = board?.stats?.byColumn ?? {}
  check(board?.stats?.total === cards.length, `stats.total 应为 ${cards.length}，实际 ${board?.stats?.total}`)
  for (const id of COLUMNS) {
    const counted = cards.filter((card) => card.column === id).length
    check(byColumn[id] === counted, `byColumn.${id} 应为 ${counted}，实际 ${byColumn[id]}`)
    check(columns[COLUMNS.indexOf(id)]?.count === counted, `columns ${id}.count 应为 ${counted}`)
  }

  const ids = new Set()
  for (const card of cards) {
    const label = `card ${card?.id ?? '(无 id)'}`
    check(isString(card.id), `${label}: id 缺失`)
    check(!ids.has(card.id), `${label}: id 重复`)
    ids.add(card.id)
    check(['project', 'goal', 'repo'].includes(card.kind), `${label}: kind 非法 ${card.kind}`)
    check(isString(card.title), `${label}: title 缺失`)
    check(COLUMNS.includes(card.column), `${label}: column 非法 ${card.column}`)
    check(typeof card.statusRaw === 'string', `${label}: statusRaw 不是字符串`)
    check(card.progress === null || (typeof card.progress === 'number' && card.progress >= 0 && card.progress <= 1), `${label}: progress 越界`)
    check(card.updated === null || (typeof card.updated === 'string' && DATE_RE.test(card.updated)), `${label}: updated 不是 YYYY-MM-DD/null`)
    check(card.ageDays === null || isInt(card.ageDays), `${label}: ageDays 不是整数/null`)
    check(typeof card.stale === 'boolean', `${label}: stale 不是布尔`)
    check(isString(card.source), `${label}: source 缺失`)
    check(Array.isArray(card.highlights), `${label}: highlights 不是数组`)
    for (const item of card.highlights ?? []) {
      check(typeof item === 'string' && item.length > 0 && item.length <= 160, `${label}: highlight 非法/超 160 字`)
    }
    if (card.progress === null) check(card.progressLabel === undefined, `${label}: progress 为 null 时不应有 progressLabel`)
    if (card.tags !== undefined) {
      check(Array.isArray(card.tags) && card.tags.length <= 6 && card.tags.every(isString), `${label}: tags 应为 ≤6 个字符串`)
    }
    if (card.subtitle !== undefined) check(isString(card.subtitle), `${label}: subtitle 应为字符串`)
    if (card.owner !== undefined) check(isString(card.owner), `${label}: owner 应为字符串`)
    if (card.latest !== undefined) check(isString(card.latest), `${label}: latest 应为字符串`)
    if (card.repo !== undefined) {
      const repo = card.repo
      check(isString(repo?.name), `${label}: repo.name 缺失`)
      check(typeof repo?.branch === 'string', `${label}: repo.branch 应为字符串`)
      check(typeof repo?.lastCommitAt === 'string' && DATE_RE.test(repo.lastCommitAt), `${label}: repo.lastCommitAt 非法`)
      check(typeof repo?.lastCommitSubject === 'string', `${label}: repo.lastCommitSubject 不是字符串`)
      check(isInt(repo?.commits30), `${label}: repo.commits30 非法`)
      check(isInt(repo?.dirty), `${label}: repo.dirty 非法`)
      check(Array.isArray(repo?.weeks) && repo.weeks.length === 8 && repo.weeks.every(isInt), `${label}: repo.weeks 应为 8 个非负整数`)
    }
  }

  // 排序：column 顺序 -> stale 靠后 -> updated 降序（无日期最后）
  const rank = new Map(COLUMNS.map((id, index) => [id, index]))
  for (let i = 1; i < cards.length; i += 1) {
    const prev = cards[i - 1]
    const next = cards[i]
    const byColumn = rank.get(prev.column) - rank.get(next.column)
    if (byColumn > 0) issues.push(`排序错误：${prev.id} 应排在 ${next.id} 之后（column）`)
    else if (byColumn === 0) {
      if (prev.stale && !next.stale) issues.push(`排序错误：${prev.id}（stale）应排在 ${next.id} 之后`)
      else if (prev.stale === next.stale) {
        if (prev.updated && next.updated && prev.updated < next.updated) {
          issues.push(`排序错误：${prev.id} 的 updated 早于 ${next.id} 却排在前面`)
        } else if (!prev.updated && next.updated) {
          issues.push(`排序错误：无 updated 的 ${prev.id} 应排在 ${next.id} 之后`)
        }
      }
    }
  }
  return issues
}

const startedAt = performance.now()
const board = await buildBoard()
const elapsedMs = performance.now() - startedAt

const byColumn = board.stats?.byColumn ?? {}
console.log('='.repeat(72))
console.log('项目看板 · Host 扫描预览')
console.log('='.repeat(72))
console.log(`generatedAt : ${board.generatedAt}`)
console.log(`扫描耗时    : ${elapsedMs.toFixed(0)} ms`)
console.log(`总卡片数    : ${board.stats.total}`)
console.log(
  `各列分布    : ${COLUMNS.map((id) => `${id}(${COLUMN_LABEL[id]}) ${byColumn[id] ?? 0}`).join(' | ')}`,
)
console.log(`columns     : ${JSON.stringify(board.columns)}`)
console.log(`warnings    : ${board.warnings.length} 条`)
for (const warning of board.warnings.slice(0, 10)) console.log(`  - ${warning}`)
if (board.warnings.length > 10) console.log(`  ... 其余 ${board.warnings.length - 10} 条省略`)

for (const id of COLUMNS) {
  const cards = board.cards.filter((card) => card.column === id)
  console.log('')
  console.log(`── ${COLUMN_LABEL[id]} / ${id}（${cards.length}）`)
  if (cards.length === 0) {
    console.log('    （空）')
    continue
  }
  for (const card of cards.slice(0, 3)) console.log(lineOf(card))
  if (cards.length > 3) console.log(`    ... 其余 ${cards.length - 3} 张省略`)
}

const sampleProject =
  board.cards.find((card) => card.kind === 'project' && card.progressLabel?.startsWith('里程碑')) ??
  board.cards.find((card) => card.kind === 'project' && card.progressLabel) ??
  board.cards.find((card) => card.kind === 'project')
const sampleRepo = board.cards.find((card) => card.kind === 'repo')

console.log('')
console.log('='.repeat(72))
console.log('完整单卡 JSON（project，便于核对 Board/Card 契约）')
console.log('='.repeat(72))
console.log(JSON.stringify(sampleProject ?? null, null, 2))

if (sampleRepo) {
  console.log('')
  console.log('='.repeat(72))
  console.log('完整单卡 JSON（repo，含 repo 字段）')
  console.log('='.repeat(72))
  console.log(JSON.stringify(sampleRepo, null, 2))
}

const issues = validateBoard(board)
console.log('')
console.log('='.repeat(72))
if (issues.length === 0) {
  console.log('契约自检：PASS（Board/Card 结构、字段类型、列顺序、排序、stats 均符合 CONTRACT.md）')
} else {
  console.log(`契约自检：FAIL（${issues.length} 处问题）`)
  for (const issue of issues) console.log(`  - ${issue}`)
}
console.log('='.repeat(72))

if (issues.length > 0 || board.stats.total === 0) process.exitCode = 1
