#!/usr/bin/env bash
# 用 macOS say 生成中文 TTS 语料（文本已知 → 可算 CER）。wav 不入库，只保留生成脚本。
# 用法：bash scripts/gen-tts-corpus.sh
set -euo pipefail
OUT="src-tauri/tests/fixtures/quality"
VOICE="Flo (中文（中国大陆）)"
mkdir -p "$OUT"

say_wav() { # $1=输出名 $2=文本
  local out="$OUT/$1.wav"
  say -v "$VOICE" -r 180 -o /tmp/quality.aiff -- "$2"
  ffmpeg -y -loglevel error -i /tmp/quality.aiff -ar 16000 -ac 1 -c:a pcm_s16le "$out"
  rm -f /tmp/quality.aiff
}

# 普通句：有参考文本，算 CER
i=0
while IFS= read -r line; do
  i=$((i+1))
  printf -v name 'tts-%02d' "$i"
  say_wav "$name" "$line"
  printf '%s\n' "$line" > "$OUT/$name.txt"
done <<'SENTENCES'
今天下午开一个产品评审会
这个功能的验收标准我们再确认一下
下周二之前把测试用例补齐
服务器昨天凌晨重启过一次
登录页面的错误提示需要改一下
这段代码的日志打得太多了
客户反馈说导出按钮点了没反应
明天下午的会议改到三点
我们先把接口文档更新一下
这个问题我这边没法复现
发布之前记得跑一遍全量测试
新版本的回滚方案准备好了吗
数据迁移脚本还需要再压测一次
前端和后端的字段命名对不上
我下午请假半天
好的没问题
再确认一下验收标准
他在会上再强调了一次交付时间
玄戒芯片的样品什么时候到
把弓头安装的图纸发我一份
SENTENCES

# 数字/英文句：只观察输出，不设 .txt（ITN 会改数字写法，算 CER 会失真）
i=0
while IFS= read -r line; do
  i=$((i+1))
  printf -v name 'num-%02d' "$i"
  say_wav "$name" "$line"
done <<'NUMERIC'
明天上午九点半在三百零一会议室开会
这个月的房租是三千八百块
订单号是二零二六零九一七
这个接口的延迟要压到一百毫秒以内
我们用 WebSocket 还是 HTTP 轮询
NUMERIC

# 长句：给 rule3 / 短句合并用
say_wav "long-01" "我们这次迭代主要做三件事情第一是修复登录页面偶发白屏的问题第二是把导出功能的超时时间从十秒调整到三十秒第三是给所有接口补上统一的错误码另外还需要在发布之前把回归测试全部跑一遍确认没有新的崩溃最后请各位在今天下班前把各自负责的部分更新到周报里"
printf '%s\n' "我们这次迭代主要做三件事情第一是修复登录页面偶发白屏的问题第二是把导出功能的超时时间从十秒调整到三十秒第三是给所有接口补上统一的错误码另外还需要在发布之前把回归测试全部跑一遍确认没有新的崩溃最后请各位在今天下班前把各自负责的部分更新到周报里" > "$OUT/long-01.txt"

# 噪声变体：真人 3 条 + TTS 2 条，混粉红噪声（≈10 dB，仅用于相对比较）
for f in real-01 real-02 real-03 tts-01 tts-02; do
  [ -f "$OUT/$f.wav" ] || continue
  ffmpeg -y -loglevel error -i "$OUT/$f.wav" -f lavfi -i "anoisesrc=color=pink:amplitude=0.02:duration=60" \
    -filter_complex "[0:a][1:a]amix=inputs=2:duration=first:dropout_transition=0" \
    -ar 16000 -ac 1 -c:a pcm_s16le "$OUT/noisy-$f.wav"
  [ -f "$OUT/$f.txt" ] && cp "$OUT/$f.txt" "$OUT/noisy-$f.txt"
done

echo "完成：$(ls "$OUT"/*.wav | wc -l | tr -d ' ') 个 wav → $OUT"
