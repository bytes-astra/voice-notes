// 拼句规则门禁自身的测试：`npm run check:join`（其中串了 `node --test` 这一步）
//
// 门禁的作用是「前后端规则漂移时红」。所以这里测的是门禁**能不能抓住漂移**——
// 光看门禁在没漂移时是绿的没用。
//
// 用 node 自带的 test runner（Node 18+），不引第三方依赖。
import { test } from 'node:test'
import assert from 'node:assert/strict'
import { readFileSync } from 'node:fs'
import { join } from 'node:path'

import { collectProblems } from './check-join.mjs'

const core = readFileSync(join(process.cwd(), 'src/core.js'), 'utf8')
const asr = readFileSync(join(process.cwd(), 'src-tauri/src/asr.rs'), 'utf8')
const audio = readFileSync(join(process.cwd(), 'src-tauri/src/audio.rs'), 'utf8')

test('没漂移时门禁放行', () => {
  const { problems, summary } = collectProblems(core, asr, audio)
  assert.deepEqual(problems, [])
  // 摘要由 collectProblems 一并返回（main 不再重算），顺手守住它的形状。
  assert.ok(summary.rangeCount > 0, '摘要里应有 CJK 区间数')
  assert.match(summary.gapShape, /^\[\[gap:<digits>\]\]$/)
})

// 规则 1/2 的判定式：前端 `isGapMarker(right) || GAP_END_RE.test(left)`，
// 后端 `is_gap_marker(part) || ends_with_gap_marker(&out)`。
test('把「或」改成「与」会被抓住（两边都改）', () => {
  const brokenCore = core.replace(
    'isGapMarker(right) || GAP_END_RE.test(left)',
    'isGapMarker(right) && GAP_END_RE.test(left)',
  )
  const brokenAsr = asr.replace(
    'is_gap_marker(part) || ends_with_gap_marker(&out)',
    'is_gap_marker(part) && ends_with_gap_marker(&out)',
  )
  const problems = collectProblems(brokenCore, brokenAsr, audio).problems
  assert.ok(problems.length > 0, '两边都改成 && 仍然一致 —— 门禁没抓住最自然的漂移')
})

test('把「或」改成「与」会被抓住（只改前端）', () => {
  const brokenCore = core.replace(
    'isGapMarker(right) || GAP_END_RE.test(left)',
    'isGapMarker(right) && GAP_END_RE.test(left)',
  )
  const problems = collectProblems(brokenCore, asr, audio).problems
  assert.ok(problems.length > 0, '只改前端为 && 仍然通过 —— 门禁漏检')
})

test('把「或」改成「与」会被抓住（只改后端）', () => {
  const brokenAsr = asr.replace(
    'is_gap_marker(part) || ends_with_gap_marker(&out)',
    'is_gap_marker(part) && ends_with_gap_marker(&out)',
  )
  const problems = collectProblems(core, brokenAsr, audio).problems
  assert.ok(problems.length > 0, '只改后端为 && 仍然通过 —— 门禁漏检')
})

test('少写半个判定条件会被抓住', () => {
  const brokenAsr = asr.replace(
    'is_gap_marker(part) || ends_with_gap_marker(&out)',
    'is_gap_marker(part)',
  )
  const problems = collectProblems(core, brokenAsr, audio).problems
  assert.ok(problems.length > 0, '少写一个判定条件仍然通过 —— 门禁漏检')
})

test('CJK 区间端点开闭漂移会被抓住', () => {
  const brokenAsr = asr.replace('0x3000..=0x303F', '0x3000..0x303F')
  const problems = collectProblems(core, brokenAsr, audio).problems
  assert.ok(problems.length > 0, '`..=` 改 `..` 仍然通过 —— 门禁漏检')
})

test('缺口占位符形状漂移会被抓住', () => {
  const brokenAudio = audio.replace(
    'GAP_MARKER_PREFIX: &str = "[[gap:"',
    'GAP_MARKER_PREFIX: &str = "[[gap"',
  )
  const problems = collectProblems(core, asr, brokenAudio).problems
  assert.ok(problems.length > 0, '占位符少个 `:` 仍然通过 —— 门禁漏检')
})

// 判定式对了不代表用法对：把三元两个分支对调，用户看到的文本就反了。
test('前端返回语句的分支对调会被抓住', () => {
  const broken = core.replace(
    'return needSpace ? `${left} ${right}` : `${left}${right}`',
    'return needSpace ? `${left}${right}` : `${left} ${right}`',
  )
  assert.notEqual(broken, core, '替换没生效 —— core.js 的返回语句写法变了？')
  const problems = collectProblems(broken, asr, audio).problems
  assert.ok(problems.length > 0, '三元分支对调仍然通过 —— 门禁没抓住')
})

test('后端空间分支对调会被抓住', () => {
  const broken = asr.replace('if need_space {', 'if !need_space {')
  assert.notEqual(broken, asr, '替换没生效 —— asr.rs 的写法变了？')
  const problems = collectProblems(core, broken, audio).problems
  assert.ok(problems.length > 0, '`if need_space` 改成 `if !need_space` 仍然通过 —— 门禁没抓住')
})
