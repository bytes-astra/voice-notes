// 抓取真人中文语音 + 逐句转写，作为转写质量语料（仅本机使用，wav 不入库）。
// 用法：node scripts/fetch-quality-corpus.mjs [条数，默认 12]
// 依赖：node 18+（fetch）、ffmpeg。
import { execFileSync } from 'node:child_process'
import { mkdirSync, writeFileSync } from 'node:fs'
import { join } from 'node:path'

const COUNT = Number(process.argv[2] || 12)
const OUT = 'src-tauri/tests/fixtures/quality'
const API = 'https://datasets-server.huggingface.co/rows?dataset=PolyAI%2Fminds14&config=zh-CN&split=train&offset=0&length='
mkdirSync(OUT, { recursive: true })

const sleep = (ms) => new Promise((r) => setTimeout(r, ms))
let rows = null
for (let attempt = 0; attempt < 5 && !rows; attempt++) {
  const res = await fetch(API + COUNT)
  if (res.ok) {
    rows = (await res.json()).rows
  } else {
    console.error(`数据集接口 ${res.status}，${attempt + 1}/5 重试…`)
    await sleep(2500)
  }
}
if (!rows) {
  console.error('拿不到数据集，放弃')
  process.exit(1)
}

let saved = 0
for (const { row } of rows) {
  const text = (row.transcription || '').trim()
  if (!text || !row.audio?.[0]?.src) continue
  saved += 1
  const id = String(saved).padStart(2, '0')
  const wav = join(OUT, `real-${id}.wav`)
  const audio = Buffer.from(await (await fetch(row.audio[0].src)).arrayBuffer())
  const tmp = join(OUT, `.real-${id}.raw.wav`)
  writeFileSync(tmp, audio)
  execFileSync('ffmpeg', ['-y', '-loglevel', 'error', '-i', tmp, '-ar', '16000', '-ac', '1', '-c:a', 'pcm_s16le', wav])
  execFileSync('rm', ['-f', tmp])
  writeFileSync(join(OUT, `real-${id}.txt`), text + '\n')
  console.log(`real-${id}.wav  ${text}`)
}
console.log(`\n完成：${saved} 条真人语音 → ${OUT}`)
