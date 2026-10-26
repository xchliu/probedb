/**
 * project-kanban · Host 数据扫描
 *
 * 纯 ESM，只依赖 node: 内置模块。导出 buildBoard(options) -> Promise<Board>。
 * 契约见 ../CONTRACT.md：扫描 aiknowledge 的项目/目标文档 + aiworkspace 下的 git 仓库，
 * 产出 Board JSON。任何单文件/单仓库失败只进 warnings，绝不抛错。
 */

import { execFile } from 'node:child_process'
import { readdir, readFile, stat } from 'node:fs/promises'
import { homedir } from 'node:os'
import { basename, join, relative, sep } from 'node:path'
import { promisify } from 'node:util'

const execFileAsync = promisify(execFile)

const COLUMNS = ['plan', 'dev', 'verify', 'deliver', 'paused']
const DAY_MS = 86_400_000
const GIT_TIMEOUT_MS = 5_000
const GIT_MAX_BUFFER = 8 * 1024 * 1024

const STALE_THRESHOLD = { plan: 30, dev: 21, verify: 21 }

/** 契约 options.excludePaths 的 scan.js 内置默认值（index.js 会显式传 config 值）。 */
const DEFAULT_EXCLUDE_PATHS = ['projects/企业AI体系建设']

const STATUS_TABLE = {
  plan: ['proposal', 'plan', 'planning', 'proposed', 'idea', 'draft', 'research', '调研', '规划', '立项', '待启动'],
  dev: ['active', 'developing', 'development', 'in_progress', 'in-progress', 'ongoing', 'wip', '进行中', '研发中', '推进中', '开发中'],
  verify: ['review', 'reviewing', 'verifying', 'testing', 'acceptance', '验收', '评审', '测试', '灰度', '内测'],
  deliver: ['done', 'completed', 'complete', 'delivered', 'shipped', 'released', 'launched', '已完成', '已交付', '已上线', '上线'],
  paused: ['paused', 'blocked', 'stopped', 'on_hold', 'hold', 'archived', 'archive', 'dropped', '暂停', '归档', '阻塞', '搁置', '停止'],
}

const EMOJI_RE =
  /[\p{Extended_Pictographic}\p{Regional_Indicator}\p{Emoji_Modifier}\uFE0F\u200D\u20E3]/gu
const CJK_RE = /[\u3400-\u9fff]/

/** 去空白/表情/markdown 噪声后的状态串，用于 status -> column 映射。 */
function normalizeStatus(raw) {
  return String(raw ?? '')
    .normalize('NFKC')
    .toLowerCase()
    .replace(EMOJI_RE, '')
    .replace(/[\s_\-–—*`"'“”‘’()（）[\]【】:：,，.。!！?？/\\|]+/g, '')
}

const KEYWORD_TO_COLUMN = new Map()
for (const column of COLUMNS) {
  for (const keyword of STATUS_TABLE[column]) {
    const key = normalizeStatus(keyword)
    if (key) KEYWORD_TO_COLUMN.set(key, column)
  }
}
const KEYWORDS_LONGEST_FIRST = [...KEYWORD_TO_COLUMN.entries()].sort((a, b) => b[0].length - a[0].length)

/** 契约 rule 2：无法识别返回 null（调用方决定默认 dev）。 */
function statusToColumn(rawStatus) {
  if (rawStatus === null || rawStatus === undefined) return null
  const status = normalizeStatus(rawStatus)
  if (!status) return null
  const exact = KEYWORD_TO_COLUMN.get(status)
  if (exact) return exact
  if (status.startsWith('dev')) return 'dev' // 契约 goals 里的 dev*
  for (const [keyword, column] of KEYWORDS_LONGEST_FIRST) {
    if (keyword.length < 2) continue
    if (CJK_RE.test(keyword)) {
      if (status.includes(keyword)) return column
    } else if (status.startsWith(keyword) || status.endsWith(keyword)) {
      return column
    }
  }
  return null
}

// ---------------------------------------------------------------- 小工具

function expandHome(input) {
  if (typeof input !== 'string') return null
  const value = input.trim()
  if (!value) return null
  if (value === '~') return homedir()
  if (value.startsWith('~/') || value.startsWith('~\\')) return join(homedir(), value.slice(2))
  return value
}

/** options.excludePaths：相对 projectsRoot 的路径前缀（默认见 DEFAULT_EXCLUDE_PATHS）。 */
function normalizeExcludePaths(raw) {
  const values = Array.isArray(raw) ? raw : DEFAULT_EXCLUDE_PATHS
  const prefixes = []
  for (const value of values) {
    if (typeof value !== 'string') continue
    const normalized = value.trim().replace(/\\/g, '/').replace(/\/+$/, '')
    if (normalized && !prefixes.includes(normalized)) prefixes.push(normalized)
  }
  return prefixes
}

function isExcludedPath(relPath, prefixes) {
  if (prefixes.length === 0) return false
  return prefixes.some((prefix) => relPath === prefix || relPath.startsWith(`${prefix}/`))
}

function fmtDate(date) {
  const y = date.getFullYear()
  const m = String(date.getMonth() + 1).padStart(2, '0')
  const d = String(date.getDate()).padStart(2, '0')
  return `${y}-${m}-${d}`
}

function parseYmd(value) {
  if (typeof value !== 'string') return null
  const match = /^(\d{4})-(\d{2})-(\d{2})$/.exec(value.trim())
  if (!match) return null
  const date = new Date(Number(match[1]), Number(match[2]) - 1, Number(match[3]))
  return Number.isNaN(date.getTime()) ? null : date
}

function normalizeYmd(value) {
  if (value === null || value === undefined) return null
  const date = parseYmd(String(value).trim().replace(/^["']|["']$/g, ''))
  return date ? fmtDate(date) : null
}

function ageDaysFrom(ymd, now) {
  const date = parseYmd(ymd)
  if (!date) return null
  const today = new Date(now.getFullYear(), now.getMonth(), now.getDate())
  const days = Math.round((today.getTime() - date.getTime()) / DAY_MS)
  return Number.isFinite(days) ? Math.max(0, days) : null
}

function computeStale(column, ageDays) {
  const threshold = STALE_THRESHOLD[column]
  if (!threshold || ageDays === null || ageDays === undefined) return false
  return ageDays > threshold
}

function stripEmoji(text) {
  return String(text).replace(EMOJI_RE, '')
}

function cleanInline(text) {
  let out = String(text ?? '')
  out = out.replace(/!\[([^\]]*)\]\([^)]*\)/g, '$1')
  out = out.replace(/\[([^\]]+)\]\([^)]*\)/g, '$1')
  out = out.replace(/\[\[([^\]|]+)(?:\|([^\]]+))?\]\]/g, (_, plain, alias) => alias || plain)
  out = out.replace(/<[^>]*>/g, ' ')
  // 只去 markdown 强调/代码标记，保留下划线（路径、文件名里有）
  out = out.replace(/[*`~]+/g, '')
  out = stripEmoji(out)
  out = out.replace(/\s+/g, ' ')
  return out.trim()
}

function truncate(text, max = 160) {
  const value = String(text ?? '').trim()
  if (value.length <= max) return value
  return `${value.slice(0, max - 1)}…`
}

function stripDatePrefix(text) {
  return String(text)
    .replace(/^\s*\d{4}-\d{2}-\d{2}\s*[：:]\s*/, '')
    .replace(/^\s*\d{1,2}月\d{1,2}日?\s*[：:]\s*/, '')
    .trim()
}

function normalizeTagsList(value) {
  if (Array.isArray(value)) return value.map((item) => String(item).trim()).filter(Boolean)
  if (typeof value !== 'string' || !value.trim()) return []
  const inner = value.trim().replace(/^\[/, '').replace(/\]$/, '')
  if (!inner) return []
  return inner
    .split(/[,，]/)
    .map((item) => item.trim().replace(/^["']|["']$/g, ''))
    .filter(Boolean)
}

// ---------------------------------------------------------------- frontmatter

function parseFrontmatter(text) {
  const source = String(text ?? '').replace(/^\uFEFF/, '')
  const lines = source.split(/\r?\n/)
  if (!/^---\s*$/.test((lines[0] ?? '').trim())) return { data: {}, body: source }
  let end = -1
  for (let i = 1; i < lines.length; i += 1) {
    if (/^---\s*$/.test(lines[i].trim())) {
      end = i
      break
    }
  }
  if (end === -1) return { data: {}, body: source }
  const data = {}
  for (let i = 1; i < end; i += 1) {
    const line = lines[i]
    if (!line.trim() || /^\s*#/.test(line)) continue
    const match = /^([^:#][^:]*):\s*(.*)$/.exec(line)
    if (!match) continue
    const key = match[1].trim().toLowerCase()
    let value = match[2].trim()
    if (
      (value.startsWith('"') && value.endsWith('"') && value.length >= 2) ||
      (value.startsWith("'") && value.endsWith("'") && value.length >= 2)
    ) {
      value = value.slice(1, -1)
    }
    data[key] = value
  }
  return { data, body: lines.slice(end + 1).join('\n') }
}

function fmGet(data, ...keys) {
  for (const key of keys) {
    const value = data[key.toLowerCase()]
    if (value !== undefined && value !== null && String(value).trim() !== '') return value
  }
  return null
}

// ---------------------------------------------------------------- 文本解析

function titleFromBody(body) {
  const match = /^#\s+(.+)$/m.exec(body)
  if (!match) return null
  const title = cleanInline(match[1])
  return title || null
}

function splitTableRow(line) {
  return String(line)
    .trim()
    .replace(/^\|/, '')
    .replace(/\|$/, '')
    .split('|')
    .map((cell) => cell.trim())
}

function looksLikeTableRow(line) {
  return /^\s*\|/.test(String(line))
}

function isSeparatorRow(line) {
  const cells = splitTableRow(line)
  // 兼容 `|:--|` 这类只写两个连字符的分隔行（treow-development.md 就是）
  return cells.length > 0 && cells.every((cell) => cell === '' || /^:?-{2,}:?$/.test(cell))
}

function isCjkKeyword(keyword) {
  return CJK_RE.test(keyword)
}

/** 契约 rule 3 第一优先级：里程碑表格。无法识别状态值的“表”不算里程碑表。 */
function classifyMilestoneValue(raw) {
  const value = String(raw ?? '').trim()
  if (!value) return null
  if (/⚠/.test(value)) return 0.5
  const normalized = normalizeStatus(value)
  if (!normalized) return null
  const zeroTokens = ['pending', 'planned', '待定', '未开始', '未启动', '未完成', 'incomplete', 'notstarted', 'todo']
  for (const token of zeroTokens) {
    if (normalized === token || (isCjkKeyword(token) && normalized.includes(token))) return 0
  }
  const doneTokens = ['done', 'completed', 'complete', '完成', '已完成', '已交付', '已上线', 'shipped', 'released', 'launched']
  for (const token of doneTokens) {
    if (normalized === token || normalized.includes(token)) return 1
  }
  const halfTokens = ['inprogress', '进行中', '推进中', '开发中', '研发中', 'active', 'review', 'testing']
  for (const token of halfTokens) {
    if (normalized === token || normalized.includes(token)) return 0.5
  }
  return null
}

function findMilestoneTable(body) {
  const lines = body.split(/\r?\n/)
  const fenced = new Array(lines.length).fill(false)
  let inFence = false
  for (let i = 0; i < lines.length; i += 1) {
    if (/^\s*(```|~~~)/.test(lines[i])) {
      inFence = !inFence
      fenced[i] = true
      continue
    }
    fenced[i] = inFence
  }
  let i = 0
  while (i < lines.length) {
    if (fenced[i] || !looksLikeTableRow(lines[i])) {
      i += 1
      continue
    }
    const block = []
    let j = i
    while (j < lines.length && !fenced[j] && looksLikeTableRow(lines[j])) {
      block.push(lines[j])
      j += 1
    }
    if (block.length >= 3 && isSeparatorRow(block[1])) {
      const headers = splitTableRow(block[0])
      const statusIndex = headers.findIndex((cell) => /状态|status|进度/i.test(cell))
      if (statusIndex !== -1) {
        const rows = block
          .slice(2)
          .map(splitTableRow)
          .filter((cells) => cells.some((cell) => cell !== ''))
        const scores = rows.map((cells) => classifyMilestoneValue(cells[statusIndex] ?? ''))
        const recognized = scores.filter((score) => score !== null).length
        if (rows.length > 0 && recognized > 0) {
          const sum = scores.reduce((acc, score) => acc + (score ?? 0), 0)
          const doneCount = scores.filter((score) => score === 1).length
          return {
            value: Math.round((sum / rows.length) * 100) / 100,
            label: `里程碑 ${doneCount}/${rows.length}`,
          }
        }
      }
    }
    i = j
  }
  return null
}

function findChecklistProgress(body) {
  const matches = String(body).match(/^\s*[-*+]\s+\[([ xX])\]/gm) ?? []
  if (matches.length === 0) return null
  const done = matches.filter((mark) => /\[[xX]\]/.test(mark)).length
  return {
    value: Math.round((done / matches.length) * 100) / 100,
    label: `任务 ${done}/${matches.length}`,
  }
}

function extractProgress(body) {
  return findMilestoneTable(body) ?? findChecklistProgress(body) ?? null
}

/** 契约 rule 4：最新一条 `**YYYY-MM-DD**：文本` / `YYYY-MM-DD：文本`。 */
function extractLatest(body) {
  const lines = body.split(/\r?\n/)
  const strict = /^[\s>*+\-]*\*\*(\d{4}-\d{2}-\d{2})\*\*[：:]\s*(.+)$/
  const loose = /^[\s>*+\-]*(\d{4}-\d{2}-\d{2})[：:]\s*(.+)$/
  let best = null
  for (const line of lines) {
    const match = strict.exec(line) ?? loose.exec(line)
    if (!match) continue
    const date = normalizeYmd(match[1])
    const text = truncate(cleanInline(match[2]), 160)
    if (!date || !text) continue
    // 日期越大越新；同一天取文档里靠后的一条
    if (!best || date >= best.date) {
      best = { date, text }
    }
  }
  return best
}

function extractSectionHighlights(body) {
  const lines = body.split(/\r?\n/)
  for (let i = 0; i < lines.length; i += 1) {
    const heading = /^##\s+(.+)$/.exec(lines[i])
    if (!heading) continue
    if (!/(研发推进|进展|进度|近况)/.test(heading[1])) continue
    const items = []
    for (let j = i + 1; j < lines.length; j += 1) {
      const line = lines[j]
      if (/^#{1,2}\s/.test(line)) break
      const bullet = /^\s*[-*+]\s+(?:\[[ xX]\]\s*)?(.*)$/.exec(line)
      if (!bullet) continue
      const text = stripDatePrefix(cleanInline(bullet[1])).replace(/^[：:]\s*/, '')
      if (!text) continue
      items.push(truncate(text, 160))
      if (items.length >= 3) break
    }
    return items
  }
  return []
}

function extractQuoteHighlights(body) {
  const lines = body.split(/\r?\n/)
  const groups = []
  let current = null
  for (const line of lines) {
    if (/^\s*>/.test(line)) {
      if (current === null) current = []
      current.push(line)
    } else if (current !== null) {
      groups.push(current)
      current = null
    }
  }
  if (current !== null) groups.push(current)

  for (const group of groups) {
    const plain = group.map((line) => cleanInline(line.replace(/^\s*>\s?/, ''))).join(' ')
    if (!/摘要|abstract|summary/i.test(plain)) continue
    const items = []
    for (const line of group) {
      const raw = line.replace(/^\s*>\s?/, '').trim()
      if (!raw || /\[![a-zA-Z]+\]/.test(raw)) continue
      const text = cleanInline(raw)
      if (!text || /^(摘要|abstract|summary)[:：]?$/i.test(text)) continue
      items.push(truncate(text, 160))
      if (items.length >= 3) break
    }
    if (items.length > 0) return items
  }
  return []
}

function extractParagraphHighlights(body) {
  const lines = body.split(/\r?\n/)
  for (const line of lines) {
    if (!line.trim()) continue
    if (/^\s*(#|>|\||```|~~~|[-*_]{3,}|<)/.test(line)) continue
    if (/^\s*[-*+]\s+/.test(line)) continue
    if (/^\s*\d+[.)]\s+/.test(line)) continue
    const text = cleanInline(line)
    if (!text) continue
    return [truncate(text, 160)]
  }
  return []
}

function extractHighlights(body) {
  const section = extractSectionHighlights(body)
  if (section.length > 0) return section
  const quote = extractQuoteHighlights(body)
  if (quote.length > 0) return quote
  return extractParagraphHighlights(body)
}

function extractOwner(frontmatter, body) {
  const fromFm = fmGet(frontmatter, 'owner', '负责人')
  if (fromFm) return cleanInline(fromFm)
  const fromBody = /^负责人[：:]\s*(\S[^\n]*)$/m.exec(body)
  if (fromBody) return cleanInline(fromBody[1])
  return null
}

function buildTags(kind, frontmatter, subtitle, extra) {
  const tags = []
  const push = (value) => {
    const text = cleanInline(String(value ?? ''))
    if (text && !tags.includes(text)) tags.push(text)
  }
  push(kind)
  for (const tag of normalizeTagsList(fmGet(frontmatter, 'tags', '标签', '关键词'))) push(tag)
  push(subtitle)
  push(extra)
  return tags.slice(0, 6)
}

// ---------------------------------------------------------------- 扫描文件

async function walkMarkdown(dir, warnings) {
  const files = []
  let entries
  try {
    entries = await readdir(dir, { withFileTypes: true })
  } catch (error) {
    warnings.push(`无法读取目录 ${dir}: ${errorMessage(error)}`)
    return files
  }
  await Promise.all(
    entries.map(async (entry) => {
      const name = entry.name
      if (name.startsWith('.')) return
      const full = join(dir, name)
      if (entry.isDirectory()) {
        if (name === 'node_modules') return
        files.push(...(await walkMarkdown(full, warnings)))
        return
      }
      if (entry.isFile() && /\.md$/i.test(name) && name.toLowerCase() !== 'index.md') {
        files.push(full)
      }
    }),
  )
  return files
}

function errorMessage(error) {
  const raw = String(error?.message ?? error ?? 'unknown error')
  return raw.split(/\r?\n/)[0]
}

async function loadDoc(file, warnings) {
  try {
    const [text, info] = await Promise.all([readFile(file, 'utf8'), stat(file)])
    return { file, text, mtime: info.mtime }
  } catch (error) {
    warnings.push(`读取失败 ${file}: ${errorMessage(error)}`)
    return null
  }
}

function relativeTo(root, file) {
  return relative(root, file).split(sep).join('/')
}

// ---------------------------------------------------------------- project 文档

function buildProjectDocCard({ file, text, mtime }, projectsRoot, now, warnings) {
  const { data: frontmatter, body } = parseFrontmatter(text)
  const rawStatus = fmGet(frontmatter, 'status', '状态')
  const type = fmGet(frontmatter, 'type')
  const isProjectType = typeof type === 'string' && type.toLowerCase() === 'project'
  if (!rawStatus && !isProjectType) return null

  const rel = relativeTo(projectsRoot, file)
  const parts = rel.split('/')
  const category = parts.length > 2 ? parts[1] : null

  let column = statusToColumn(rawStatus)
  let statusRaw = rawStatus ? String(rawStatus).trim() : ''
  if (!column) {
    if (rawStatus) {
      column = 'dev' // 契约 rule 2：无法识别 -> dev
    } else {
      const fileAge = ageDaysFrom(fmtDate(mtime), now)
      column = (fileAge ?? 999) <= 30 ? 'dev' : 'paused'
      statusRaw = 'type:project'
    }
  }

  const latest = extractLatest(body)
  const fmUpdated = normalizeYmd(fmGet(frontmatter, 'updated', '更新日期'))
  const updated = fmUpdated ?? latest?.date ?? fmtDate(mtime)
  const ageDays = ageDaysFrom(updated, now)

  const progress = extractProgress(body)
  const product = fmGet(frontmatter, '产品')
  const subtitle = product ? cleanInline(product) : category

  const title =
    cleanInline(fmGet(frontmatter, 'title') ?? '') || titleFromBody(body) || basename(file, '.md')

  const card = {
    id: rel,
    kind: 'project',
    title,
    column,
    statusRaw,
    progress: progress ? progress.value : null,
    updated,
    ageDays,
    stale: computeStale(column, ageDays),
    highlights: extractHighlights(body),
    tags: buildTags('project', frontmatter, subtitle, fmGet(frontmatter, '领域')),
    source: rel,
  }
  if (subtitle) card.subtitle = subtitle
  const owner = extractOwner(frontmatter, body)
  if (owner) card.owner = owner
  if (latest) card.latest = `${latest.date}：${latest.text}`
  if (progress) card.progressLabel = progress.label
  return { card, body }
}

// ---------------------------------------------------------------- goal 文档

function buildGoalDocCard({ file, text, mtime }, projectsRoot, now) {
  const { data: frontmatter, body } = parseFrontmatter(text)
  const rawStatus = fmGet(frontmatter, 'status', '状态')
  const rel = relativeTo(projectsRoot, file)
  const column = statusToColumn(rawStatus) ?? 'dev'
  const statusRaw = rawStatus ? String(rawStatus).trim() : ''

  const latest = extractLatest(body)
  const fmUpdated = normalizeYmd(fmGet(frontmatter, 'updated', '更新日期'))
  const updated = fmUpdated ?? latest?.date ?? fmtDate(mtime)
  const ageDays = ageDaysFrom(updated, now)
  const progress = extractProgress(body)

  const title =
    cleanInline(fmGet(frontmatter, 'title') ?? '') || titleFromBody(body) || basename(file, '.md')
  const subtitle = '目标'

  const card = {
    id: rel,
    kind: 'goal',
    title,
    column,
    statusRaw,
    progress: progress ? progress.value : null,
    updated,
    ageDays,
    stale: computeStale(column, ageDays),
    highlights: extractHighlights(body),
    tags: buildTags('goal', frontmatter, subtitle),
    source: rel,
  }
  if (subtitle) card.subtitle = subtitle
  const owner = extractOwner(frontmatter, body)
  if (owner) card.owner = owner
  if (latest) card.latest = `${latest.date}：${latest.text}`
  if (progress) card.progressLabel = progress.label
  return { card, body }
}

// ---------------------------------------------------------------- git 仓库

async function runGit(dir, args, warnings, label) {
  try {
    const { stdout } = await execFileAsync('git', ['-C', dir, ...args], {
      timeout: GIT_TIMEOUT_MS,
      maxBuffer: GIT_MAX_BUFFER,
      windowsHide: true,
    })
    return stdout
  } catch (error) {
    warnings.push(`仓库 ${label}: git ${args[0]} 失败 (${errorMessage(error)})`)
    return null
  }
}

/** ISO 周编号 key：YYYY-Www。 */
function isoWeekKey(date) {
  const d = new Date(Date.UTC(date.getFullYear(), date.getMonth(), date.getDate()))
  const day = d.getUTCDay() || 7
  d.setUTCDate(d.getUTCDate() + 4 - day)
  const yearStart = new Date(Date.UTC(d.getUTCFullYear(), 0, 1))
  const week = Math.ceil(((d.getTime() - yearStart.getTime()) / DAY_MS + 1) / 7)
  return `${d.getUTCFullYear()}-W${String(week).padStart(2, '0')}`
}

function recentWeekKeys(now) {
  const monday = new Date(now.getFullYear(), now.getMonth(), now.getDate())
  const offset = (monday.getDay() + 6) % 7
  monday.setDate(monday.getDate() - offset)
  const keys = []
  for (let i = 7; i >= 0; i -= 1) {
    const weekStart = new Date(monday.getFullYear(), monday.getMonth(), monday.getDate() - i * 7)
    keys.push(isoWeekKey(weekStart))
  }
  return keys
}

function repoColumnFor(lastCommitAt, subject, now) {
  const ageDays = ageDaysFrom(lastCommitAt, now) ?? 999
  const archived = typeof subject === 'string' && /archive|归档|暂停/i.test(subject)
  if (archived) return { column: 'paused', ageDays }
  if (ageDays <= 14) return { column: 'dev', ageDays }
  return { column: 'paused', ageDays }
}

async function scanRepos({ reposRoot, repoAllow, repoDeny, now, warnings }) {
  const cards = []
  const byName = new Map()
  const allow = new Set(Array.isArray(repoAllow) ? repoAllow.map((name) => String(name)) : [])
  const deny = new Set(Array.isArray(repoDeny) ? repoDeny.map((name) => String(name)) : [])

  let entries
  try {
    entries = await readdir(reposRoot, { withFileTypes: true })
  } catch (error) {
    warnings.push(`无法读取仓库根目录 ${reposRoot}: ${errorMessage(error)}`)
    return { cards, byName }
  }

  const candidates = entries
    .filter((entry) => entry.isDirectory() && !entry.name.startsWith('.'))
    .map((entry) => entry.name)
    .filter((name) => {
      if (allow.size > 0 && !allow.has(name)) return false
      if (deny.has(name)) return false
      return true
    })

  const weekKeys = recentWeekKeys(now)
  const weekIndex = new Map(weekKeys.map((key, index) => [key, index]))

  await Promise.all(
    candidates.map(async (name) => {
      const dir = join(reposRoot, name)
      try {
        const info = await stat(join(dir, '.git'))
        if (!info) return
      } catch {
        return // 不是 git 仓库，按契约跳过（不产生 warning）
      }

      const [lastLog, log30, statusOut, branchOut, weekLog] = await Promise.all([
        runGit(dir, ['log', '-1', '--format=%ad|%s', '--date=short'], warnings, name),
        runGit(dir, ['log', '--since=30 days ago', '--oneline'], warnings, name),
        runGit(dir, ['status', '--porcelain'], warnings, name),
        runGit(dir, ['rev-parse', '--abbrev-ref', 'HEAD'], warnings, name),
        runGit(dir, ['log', '--since=8 weeks ago', '--format=%ad', '--date=short'], warnings, name),
      ])

      const firstLine = String(lastLog ?? '').split(/\r?\n/).find((line) => line.trim()) ?? ''
      const separator = firstLine.indexOf('|')
      const lastCommitAt = normalizeYmd(separator === -1 ? '' : firstLine.slice(0, separator))
      const lastCommitSubject = separator === -1 ? '' : firstLine.slice(separator + 1).trim()
      if (!lastCommitAt) {
        warnings.push(`仓库 ${name}: 无提交记录，已跳过`)
        return
      }

      const commits30 = String(log30 ?? '')
        .split(/\r?\n/)
        .filter((line) => line.trim()).length
      const dirty = String(statusOut ?? '')
        .split(/\r?\n/)
        .filter((line) => line.trim()).length
      const branch = String(branchOut ?? '').trim()

      const weeks = new Array(8).fill(0)
      for (const line of String(weekLog ?? '').split(/\r?\n/)) {
        const ymd = normalizeYmd(line)
        if (!ymd) continue
        const date = parseYmd(ymd)
        if (!date) continue
        const index = weekIndex.get(isoWeekKey(date))
        if (index !== undefined) weeks[index] += 1
      }

      const { column, ageDays } = repoColumnFor(lastCommitAt, lastCommitSubject, now)
      const highlights = []
      if (ageDays > 0) highlights.push(`${ageDays} 天无提交`)
      if (/archive|归档|暂停/i.test(lastCommitSubject)) highlights.push('最近提交标记归档/暂停')
      if (commits30 > 0) highlights.push(`近 30 天提交 ${commits30} 次`)
      if (dirty > 0) highlights.push(`${dirty} 个未提交变更`)
      if (branch) highlights.push(`分支 ${branch}`)

      const repo = {
        name,
        branch,
        lastCommitAt,
        lastCommitSubject,
        commits30,
        dirty,
        weeks,
      }
      const card = {
        id: `repo:${name}`,
        kind: 'repo',
        title: name,
        subtitle: '代码库',
        statusRaw: column === 'dev' ? 'active' : 'paused',
        column,
        progress: null,
        updated: lastCommitAt,
        ageDays,
        stale: computeStale(column, ageDays),
        highlights: highlights.slice(0, 3).map((item) => truncate(item, 160)),
        tags: ['repo'],
        source: name,
        repo,
      }
      cards.push(card)
      byName.set(name, repo)
    }),
  )

  return { cards, byName }
}

// ---------------------------------------------------------------- repo 关联

const REPO_PATH_RE = /(?:~\/Documents\/aiworkspace\/|aiworkspace\/)([A-Za-z0-9._-]+)/g

function attachRelatedRepos(cards, repoByName) {
  if (repoByName.size === 0) return
  for (const item of cards) {
    const repo = repoByName.get(item.card.id.replace(/^repo:/, ''))
    if (repo) continue // repo 卡片本身在 buildRepoCard 里已经带好
    const text = item.body
    REPO_PATH_RE.lastIndex = 0
    let match
    while ((match = REPO_PATH_RE.exec(text)) !== null) {
      const name = match[1]
      if (!repoByName.has(name)) continue
      item.card.repo = repoByName.get(name)
      break
    }
  }
}

// ---------------------------------------------------------------- 同题项目去重

/** 同一 title 的项目卡只留一张：优先非 /docs/、其次路径更短、再次字典序更小。 */
function prefersProjectCopy(candidate, incumbent) {
  const candidateInDocs = candidate.card.id.includes('/docs/')
  const incumbentInDocs = incumbent.card.id.includes('/docs/')
  if (candidateInDocs !== incumbentInDocs) return candidateInDocs ? 1 : -1
  const byLength = candidate.card.id.length - incumbent.card.id.length
  if (byLength !== 0) return byLength
  return candidate.card.id < incumbent.card.id ? -1 : candidate.card.id > incumbent.card.id ? 1 : 0
}

/** 只对 kind === 'project' 生效；repo/goal 不参与。被合并数量汇总成一条 warning。 */
function dedupeProjectCards(items, warnings) {
  const keptByTitle = new Map()
  const result = []
  let dropped = 0
  for (const item of items) {
    if (item.card.kind !== 'project') {
      result.push(item)
      continue
    }
    const title = item.card.title.trim()
    const incumbent = keptByTitle.get(title)
    if (!incumbent) {
      keptByTitle.set(title, item)
      result.push(item)
      continue
    }
    dropped += 1
    if (prefersProjectCopy(item, incumbent) < 0) {
      const index = result.indexOf(incumbent)
      if (index !== -1) result[index] = item
      keptByTitle.set(title, item)
    }
  }
  if (dropped > 0) warnings.push(`已合并 ${dropped} 张重复项目卡片`)
  return result
}

// ---------------------------------------------------------------- 排序

function sortCards(cards) {
  const rank = new Map(COLUMNS.map((column, index) => [column, index]))
  return cards.sort((a, b) => {
    const byColumn = (rank.get(a.column) ?? 99) - (rank.get(b.column) ?? 99)
    if (byColumn !== 0) return byColumn
    if (a.stale !== b.stale) return a.stale ? 1 : -1
    if (a.updated && b.updated) {
      const byUpdated = b.updated.localeCompare(a.updated)
      if (byUpdated !== 0) return byUpdated
    } else if (a.updated) {
      return -1
    } else if (b.updated) {
      return 1
    }
    const byTitle = a.title.localeCompare(b.title, 'zh-Hans-CN')
    return byTitle !== 0 ? byTitle : a.id.localeCompare(b.id)
  })
}

function emptyBoard(generatedAt, warnings) {
  const byColumn = {}
  for (const column of COLUMNS) byColumn[column] = 0
  return {
    generatedAt,
    warnings,
    stats: { total: 0, byColumn },
    columns: COLUMNS.map((id) => ({ id, count: 0 })),
    cards: [],
  }
}

function buildBoardResult(items, now, warnings) {
  const cards = items.map((item) => item.card)
  sortCards(cards)
  const byColumn = {}
  for (const column of COLUMNS) byColumn[column] = 0
  for (const card of cards) byColumn[card.column] = (byColumn[card.column] ?? 0) + 1
  return {
    generatedAt: now.toISOString(),
    warnings,
    stats: { total: cards.length, byColumn },
    columns: COLUMNS.map((id) => ({ id, count: byColumn[id] })),
    cards,
  }
}

// ---------------------------------------------------------------- 入口

/**
 * @param {{
 *   projectsRoot?: string,
 *   reposRoot?: string,
 *   now?: Date,
 *   repoAllow?: string[],
 *   repoDeny?: string[],
 *   excludePaths?: string[],
 * }} [options]
 * @returns {Promise<object>} Board
 */
export async function buildBoard(options = {}) {
  const warnings = []
  const now =
    options && options.now instanceof Date && !Number.isNaN(options.now.getTime())
      ? options.now
      : new Date()

  const projectsRoot = expandHome(options?.projectsRoot) ?? join(homedir(), 'Documents', 'aiworkspace', 'aiknowledge')
  const reposRoot = expandHome(options?.reposRoot) ?? join(homedir(), 'Documents', 'aiworkspace')
  const excludePrefixes = normalizeExcludePaths(options?.excludePaths)

  try {
    const projectsDir = join(projectsRoot, 'projects')
    const goalsDir = join(projectsRoot, 'goals')

    const [projectFiles, goalFiles, repoScan] = await Promise.all([
      walkMarkdown(projectsDir, warnings),
      walkMarkdown(goalsDir, warnings),
      scanRepos({
        reposRoot,
        repoAllow: options?.repoAllow,
        repoDeny: options?.repoDeny,
        now,
        warnings,
      }),
    ])

    const visibleProjectFiles = projectFiles.filter(
      (file) => !isExcludedPath(relativeTo(projectsRoot, file), excludePrefixes),
    )

    const [projectDocs, goalDocs] = await Promise.all([
      Promise.all(visibleProjectFiles.map((file) => loadDoc(file, warnings))),
      Promise.all(goalFiles.map((file) => loadDoc(file, warnings))),
    ])

    const items = []
    for (const doc of projectDocs) {
      if (!doc) continue
      try {
        const built = buildProjectDocCard(doc, projectsRoot, now, warnings)
        if (built) items.push(built)
      } catch (error) {
        warnings.push(`解析项目文档失败 ${doc.file}: ${errorMessage(error)}`)
      }
    }
    for (const doc of goalDocs) {
      if (!doc) continue
      try {
        items.push(buildGoalDocCard(doc, projectsRoot, now))
      } catch (error) {
        warnings.push(`解析目标文档失败 ${doc.file}: ${errorMessage(error)}`)
      }
    }
    for (const card of repoScan.cards) items.push({ card, body: '' })

    const dedupedItems = dedupeProjectCards(items, warnings)
    attachRelatedRepos(dedupedItems, repoScan.byName)
    return buildBoardResult(dedupedItems, now, warnings)
  } catch (error) {
    warnings.push(`扫描失败: ${errorMessage(error)}`)
    return emptyBoard(now.toISOString(), warnings)
  }
}

export default buildBoard
