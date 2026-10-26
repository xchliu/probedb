import { readFile } from 'node:fs/promises'
import { dirname, join } from 'node:path'
import { fileURLToPath } from 'node:url'
import { buildBoard } from './host/scan.js'

const HERE = dirname(fileURLToPath(import.meta.url))
const ROUTE = '/kanban/board.json'
const CACHE_MS = 20_000

export const inject = ['webServer']

export function apply(ctx) {
  let cache = null
  let cachedAt = 0
  let inflight = null

  async function readConfig() {
    try {
      const raw = await readFile(join(HERE, 'config.json'), 'utf8')
      const parsed = JSON.parse(raw)
      return parsed !== null && typeof parsed === 'object' ? parsed : {}
    } catch {
      return {}
    }
  }

  async function build(force) {
    if (!force && cache !== null && Date.now() - cachedAt < CACHE_MS) return cache
    if (inflight !== null) return inflight
    inflight = (async () => {
      const config = await readConfig()
      const board = await buildBoard({
        projectsRoot: config.projectsRoot,
        reposRoot: config.reposRoot,
        repoAllow: config.repoAllow,
        repoDeny: config.repoDeny,
        excludePaths: config.excludePaths,
      })
      cache = board
      cachedAt = Date.now()
      return board
    })().finally(() => {
      inflight = null
    })
    return inflight
  }

  ctx.effect(
    () =>
      ctx.webServer.register({
        kind: 'exact',
        path: ROUTE,
        handler: async (req, res) => {
          let url
          try {
            url = new URL(req.url ?? ROUTE, 'http://localhost')
          } catch {
            url = new URL(ROUTE, 'http://localhost')
          }
          try {
            const body = JSON.stringify(await build(url.searchParams.has('refresh')))
            res.statusCode = 200
            res.setHeader('content-type', 'application/json; charset=utf-8')
            res.setHeader('cache-control', 'no-store')
            res.end(body)
          } catch (error) {
            res.statusCode = 500
            res.setHeader('content-type', 'application/json; charset=utf-8')
            res.end(JSON.stringify({ error: String(error?.message ?? error) }))
          }
        },
      }),
    'project-kanban: board route',
  )
}
