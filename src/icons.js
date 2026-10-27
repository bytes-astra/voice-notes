/* 图标：内联 SVG 字符串。
 *
 * 这里是图标的唯一来源：历史记录的行内按钮和转写结果卡片右上角的按钮
 * 都从这里取 —— 同一段 SVG 不在 HTML 和 JS 里各写一份。
 *
 * 约定：viewBox 一律 "0 0 24 24"，尺寸 16×16，描边用 currentColor
 * （跟着主题走）。图形本身 aria-hidden —— 按钮的可访问名称由按钮自己的
 * aria-label 提供。
 */

/// 复制：两张叠在一起的纸。
export function copyIcon() {
  return '<svg viewBox="0 0 24 24" width="16" height="16" aria-hidden="true"><rect x="9" y="9" width="11" height="11" rx="2.5" fill="none" stroke="currentColor" stroke-width="1.7"/><path d="M15 5.5A2.5 2.5 0 0 0 12.5 3H6.5A2.5 2.5 0 0 0 4 5.5v6A2.5 2.5 0 0 0 6.5 14" fill="none" stroke="currentColor" stroke-width="1.7"/></svg>'
}

/// 保存：向下箭头落进托盘。
export function saveIcon() {
  return '<svg viewBox="0 0 24 24" width="16" height="16" aria-hidden="true"><path d="M12 4v10m0 0 3.5-3.5M12 14l-3.5-3.5M5 16v3h14v-3" fill="none" stroke="currentColor" stroke-width="1.7" stroke-linecap="round" stroke-linejoin="round"/></svg>'
}

/// 删除：垃圾桶。
export function trashIcon() {
  return '<svg viewBox="0 0 24 24" width="16" height="16" aria-hidden="true"><path d="M5 7h14M10 7V5h4v2m-7 0 .8 12h8.4L17 7" fill="none" stroke="currentColor" stroke-width="1.7" stroke-linecap="round" stroke-linejoin="round"/></svg>'
}

/// 清空：橡皮擦。
/// 跟垃圾桶分开：垃圾桶是「删掉这条历史」（不可恢复），橡皮擦是「抹掉这段文字」。
export function clearIcon() {
  return '<svg viewBox="0 0 24 24" width="16" height="16" aria-hidden="true"><path d="m7 21-4.3-4.3c-1-1-1-2.5 0-3.4l9.6-9.6c1-1 2.5-1 3.4 0l5.6 5.6c1 1 1 2.5 0 3.4L13 21" fill="none" stroke="currentColor" stroke-width="1.7" stroke-linecap="round" stroke-linejoin="round"/><path d="M22 21H7" fill="none" stroke="currentColor" stroke-width="1.7" stroke-linecap="round"/><path d="m5 11 9 9" fill="none" stroke="currentColor" stroke-width="1.7" stroke-linecap="round"/></svg>'
}
