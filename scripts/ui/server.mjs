// UI 检查用的静态服务器：直接伺服仓库里的 src/。
//
// **别用 `python -m http.server`**：那个会偶发 `ERR_CONNECTION_RESET`
// （app.js / i18n.js 加载失败），表现是 `state is not defined` 或半渲染，
// 很容易误判成代码问题。
import http from 'node:http'
import { readFile } from 'node:fs/promises'
import { extname, join, normalize } from 'node:path'
import { fileURLToPath } from 'node:url'

const TYPES = {
  '.html': 'text/html; charset=utf-8',
  '.js': 'text/javascript; charset=utf-8',
  '.css': 'text/css; charset=utf-8',
  '.png': 'image/png',
  '.svg': 'image/svg+xml',
}

/// 仓库根目录（scripts/ui/ 的上上级）
export const REPO = join(fileURLToPath(new URL('../..', import.meta.url)))
export const SRC = join(REPO, 'src')

/// 起一个静态服务器，返回它的地址。
export async function startServer(root = SRC) {
  const server = http.createServer(async (req, res) => {
    let path = decodeURIComponent(new URL(req.url, 'http://localhost').pathname)
    if (path === '/') path = '/index.html'
    const file = normalize(join(root, path))
    if (!file.startsWith(root)) {
      res.writeHead(403).end('forbidden')
      return
    }
    try {
      const data = await readFile(file)
      res.writeHead(200, { 'content-type': TYPES[extname(file)] || 'application/octet-stream' })
      res.end(data)
    } catch {
      res.writeHead(404).end('not found')
    }
  })
  await new Promise((resolve) => server.listen(0, '127.0.0.1', resolve))
  return { server, base: `http://127.0.0.1:${server.address().port}` }
}
