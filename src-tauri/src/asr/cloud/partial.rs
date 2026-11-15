//! 中间结果归一器：把各家的协议形态归一成「当前句的累计串」。
//!
//! 上层（会话层 / 前端）只认一种语义：**当前未定稿句的累计文本**（实时行是替换语义）。
//! 各家协议形态不同，这里把差异关住：
//!
//! - 增量拼接（OpenAI Realtime 的 `.delta`）：每次追加一段；
//! - 确认前缀加草稿（百炼的 `.text` + `stash`）：整体替换成「确认前缀 + 草稿」；
//! - 区间替换（讯飞的 `pgs` + `rg`）：按区间改写已识别的片段。
//!
//! **以句为界**：每个句子一个桶，句定稿（final / failed）时清掉它自己的桶。
//! 没有句标识的协议（服务端断句族）走**单桶**：一条定稿清空整桶，且不支持跨句交错。

use super::{PartialSemantics, SentenceId};

/// 一次中间结果更新（对应协议侧的一次增量）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PartialUpdate {
    /// 增量拼接：把这段接到当前句末尾。
    Delta(String),
    /// 确认前缀 + 草稿：整句当前预览（确认部分不会再变，草稿可能变）。
    ///
    /// 目前只有增量形态的 provider 在用；另两种形态是按各家协议预留的
    /// （百炼的 `.text`+`stash`、讯飞的 `pgs`+`rg`），行为有单测覆盖。
    #[allow(dead_code)]
    Preview { confirmed: String, draft: String },
    /// 区间替换：把当前句第 `from..=to` 个片段替换成 `text`。
    #[allow(dead_code)]
    Replace { from: usize, to: usize, text: String },
}

/// 一个句子的累计状态：按识别顺序保存片段。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct SentenceState {
    /// 已识别的片段（按顺序）。增量与区间替换都靠它。
    pieces: Vec<String>,
    /// 确认前缀 + 草稿形态：最后一次整体预览。
    preview: Option<String>,
    /// 最近一次更新的时刻（单调递增计数）：用来判断「当前活跃句」。
    touched: u64,
}

impl SentenceState {
    /// 这一句当前的累计文本。
    fn text(&self) -> String {
        match &self.preview {
            Some(preview) => preview.clone(),
            None => self.pieces.concat(),
        }
    }
}

/// 中间结果归一器。
#[derive(Debug)]
pub struct PartialAccumulator {
    semantics: PartialSemantics,
    /// 句子桶。键为句标识；`None` 表示「没有句标识的单桶」。
    buckets: Vec<(SentenceId, SentenceState)>,
    /// 单调递增的更新计数（用来挑「当前活跃句」）。
    clock: u64,
}

impl PartialAccumulator {
    /// 新建一个归一器，按语义形态分派。
    pub fn new(semantics: PartialSemantics) -> Self {
        Self {
            semantics,
            buckets: Vec::new(),
            clock: 0,
        }
    }

    /// 应用一次更新，返回该句当前的累计串。
    ///
    /// `id` 为句标识；`None` 表示协议没给句标识（走单桶）。
    pub fn apply(&mut self, id: SentenceId, update: PartialUpdate) -> String {
        self.clock += 1;
        let index = self.bucket_index(id.clone());
        let state = &mut self.buckets[index].1;
        state.touched = self.clock;
        match (&self.semantics, update) {
            (PartialSemantics::Delta, PartialUpdate::Delta(delta)) => {
                state.pieces.push(delta);
            }
            (PartialSemantics::Preview, PartialUpdate::Preview { confirmed, draft }) => {
                state.preview = Some(format!("{confirmed}{draft}"));
            }
            (PartialSemantics::Range, PartialUpdate::Replace { from, to, text }) => {
                let last = state.pieces.len().saturating_sub(1);
                let from = from.min(last);
                let to = to.min(last).max(from);
                state.pieces.splice(from..=to, std::iter::once(text));
            }
            // 形态与更新不匹配（例如增量协议却收到区间替换）：按收到的更新处理，
            // 不静默丢弃 —— 上层宁可看到略偏的文本，也不要没有文本。
            (_, PartialUpdate::Delta(delta)) => state.pieces.push(delta),
            (_, PartialUpdate::Preview { confirmed, draft }) => {
                state.preview = Some(format!("{confirmed}{draft}"));
            }
            (_, PartialUpdate::Replace { from, to, text }) => {
                let last = state.pieces.len().saturating_sub(1);
                let from = from.min(last);
                let to = to.min(last).max(from);
                state.pieces.splice(from..=to, std::iter::once(text));
            }
        }
        self.buckets[index].1.text()
    }

    /// 某一句定稿（final 或 failed）：清掉它自己的桶，返回它最后的累计串。
    ///
    /// 没有句标识时（单桶）：清空整桶。
    pub fn finish_sentence(&mut self, id: SentenceId) -> Option<String> {
        let index = self.bucket_index(id);
        let state = self.buckets.remove(index);
        let text = state.1.text();
        if text.trim().is_empty() {
            None
        } else {
            Some(text)
        }
    }

    /// 当前活跃句的累计串（最近一次被更新的那一句）。
    ///
    /// 用于「旧句定稿后补发活跃句的实时文本」——前端在收到任何定稿事件时会清空
    /// 实时行，迟到的旧句定稿不该把新句正在显示的内容一起清掉。
    pub fn current(&self) -> Option<String> {
        self.buckets
            .iter()
            .max_by_key(|(_, state)| state.touched)
            .map(|(_, state)| state.text())
            .filter(|text| !text.trim().is_empty())
    }

    /// 找到（或建出）某个句标识对应的桶下标。
    fn bucket_index(&mut self, id: SentenceId) -> usize {
        if let Some(position) = self
            .buckets
            .iter()
            .position(|(bucket_id, _)| *bucket_id == id)
        {
            return position;
        }
        // 没有句标识 → 单桶：复用已有的那个无标识桶，不新建。
        if id.is_none() {
            if let Some(position) = self
                .buckets
                .iter()
                .position(|(bucket_id, _)| bucket_id.is_none())
            {
                return position;
            }
        }
        self.buckets.push((id, SentenceState::default()));
        self.buckets.len() - 1
    }
}

#[cfg(test)]
mod tests;
