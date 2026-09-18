//! 过滤掉模型吐在正文里的「思考」段。
//!
//! 两种风格：
//! - GLM 这类走独立字段 `reasoning_content`，`llm.rs` 只取 `content`，天然不受影响；
//! - MiniMax、Qwen 这类直接把 `<think>…</think>` 塞进 `content`，必须在流里剪掉。
//!
//! 难点在于流式：标签可能被切成 `<thi` + `nk>` 分两个分片到达，所以要留一小段
//! 尾巴等下一片，不能见字就往外吐。

/// 认的开标签。闭标签是对应的 `</…>`。
const OPEN_TAGS: [&str; 2] = ["<think>", "<thinking>"];

/// 流式的 `<think>` 剪除器。对每个模型各持有一个实例。
#[derive(Default)]
pub struct ThinkFilter {
    /// 正在思考段内部，正文要丢弃。
    in_think: bool,
    /// 攒着可能被截断的标签尾巴。
    pending: String,
    /// 是否已经吐出过正文。思考段剪掉后常跟着一串空行（MiniMax 爱写
    /// `<think>…</think>\n\n\n答案`），正文开头不该有它们。
    started: bool,
    /// 尾部空白先扣着不吐：后面若还有正文就连着放出去，流结束就丢弃。
    trailing_ws: String,
}

impl ThinkFilter {
    /// 喂一段增量，返回应当展示给用户的部分（可能是空串）。
    pub fn feed(&mut self, chunk: &str) -> String {
        self.pending.push_str(chunk);
        let mut out = String::new();

        loop {
            if self.in_think {
                let Some((idx, tag)) = first_match(&self.pending, &close_tags()) else {
                    // 还没见到闭标签：丢掉肯定不是标签前缀的部分，剩下的留着等下一片。
                    let keep = longest_suffix_prefix(&self.pending, &close_tags());
                    self.pending = self.pending[self.pending.len() - keep..].to_string();
                    break;
                };
                self.pending = self.pending[idx + tag.len()..].to_string();
                self.in_think = false;
            } else {
                let Some((idx, tag)) = first_match(&self.pending, &OPEN_TAGS) else {
                    // 没有开标签：除了可能是标签开头的尾巴，其余都能安全输出。
                    let keep = longest_suffix_prefix(&self.pending, &OPEN_TAGS);
                    let cut = self.pending.len() - keep;
                    out.push_str(&self.pending[..cut]);
                    self.pending = self.pending[cut..].to_string();
                    break;
                };
                out.push_str(&self.pending[..idx]);
                self.pending = self.pending[idx + tag.len()..].to_string();
                self.in_think = true;
            }
        }

        self.trim_edges(&mut out);
        out
    }

    /// 首尾空白裁剪。
    ///
    /// - 开头：正文还没开始时把空白直接丢掉——那是思考段留下的空行，
    ///   会让结果区在「模型名 → 正文」之间顶出一大块空白。
    /// - 结尾：空白先扣在 `trailing_ws` 里，后面来了正文就连着放出去
    ///   （中间的段落间距不受影响），流结束了就丢弃。
    /// - 中间的空白一律原样保留。
    fn trim_edges(&mut self, out: &mut String) {
        if !self.started {
            let trimmed = out.trim_start();
            if trimmed.is_empty() {
                out.clear();
                return;
            }
            *out = trimmed.to_string();
            self.started = true;
        }

        let body = out.trim_end();
        if body.is_empty() {
            // 整段都是空白：扣下，等下一片看还有没有正文。
            self.trailing_ws.push_str(out);
            out.clear();
            return;
        }
        let head = std::mem::take(&mut self.trailing_ws);
        self.trailing_ws = out[body.len()..].to_string();
        *out = format!("{head}{body}");
    }

    /// 流结束时调用：把还扣着的尾巴放出来（除非它还在思考段里）。
    pub fn finish(&mut self) -> String {
        // 尾部空白到流结束都没等到后续正文，丢弃。
        self.trailing_ws.clear();
        if self.in_think {
            self.pending.clear();
            return String::new();
        }
        if !self.started {
            return std::mem::take(&mut self.pending).trim_start().to_string();
        }
        std::mem::take(&mut self.pending)
    }
}

fn close_tags() -> [&'static str; 2] {
    ["</think>", "</thinking>"]
}

/// 找最靠前的一个标签，返回 (字节下标, 命中的标签)。
fn first_match<'a>(haystack: &str, tags: &[&'a str]) -> Option<(usize, &'a str)> {
    tags.iter()
        .filter_map(|t| haystack.find(t).map(|i| (i, *t)))
        .min_by_key(|(i, _)| *i)
}

/// 末尾有多少字节可能是某个标签被截断的开头。这部分不能输出，得留到下一片。
fn longest_suffix_prefix(haystack: &str, tags: &[&str]) -> usize {
    let max = tags.iter().map(|t| t.len()).max().unwrap_or(0).min(haystack.len());
    for len in (1..=max).rev() {
        let start = haystack.len() - len;
        // 必须落在字符边界上，否则切出来的不是合法 UTF-8。
        if !haystack.is_char_boundary(start) {
            continue;
        }
        let suffix = &haystack[start..];
        if tags.iter().any(|t| t.starts_with(suffix)) {
            return len;
        }
    }
    0
}

#[cfg(test)]
mod tests {
    use super::ThinkFilter;

    fn run(chunks: &[&str]) -> String {
        let mut f = ThinkFilter::default();
        let mut out = String::new();
        for c in chunks {
            out.push_str(&f.feed(c));
        }
        out.push_str(&f.finish());
        out
    }

    #[test]
    fn strips_whole_think_block() {
        assert_eq!(run(&["<think>盘算一下</think>河岸边的狐狸"]), "河岸边的狐狸");
    }

    #[test]
    fn strips_block_split_across_chunks() {
        // 标签被切成三片，仍然要完整识别
        assert_eq!(run(&["<thi", "nk>盘算", "一下</thi", "nk>译文"]), "译文");
    }

    #[test]
    fn keeps_plain_text_untouched() {
        assert_eq!(run(&["一只", "敏捷的狐狸"]), "一只敏捷的狐狸");
    }

    #[test]
    fn keeps_text_before_and_after() {
        assert_eq!(run(&["前言<think>思考</think>后文"]), "前言后文");
    }

    #[test]
    fn handles_thinking_variant() {
        assert_eq!(run(&["<thinking>x</thinking>y"]), "y");
    }

    #[test]
    fn unterminated_think_is_dropped() {
        // 模型只吐了半截思考就断流，不该把思考内容漏出来
        assert_eq!(run(&["<think>还没说完"]), "");
    }

    #[test]
    fn lone_angle_bracket_is_not_swallowed() {
        assert_eq!(run(&["a < b"]), "a < b");
    }

    #[test]
    fn multibyte_boundary_is_safe() {
        // 尾巴切在多字节字符中间时不能 panic
        assert_eq!(run(&["译文：中文内容"]), "译文：中文内容");
    }

    #[test]
    fn drops_blank_lines_after_think_block() {
        // MiniMax 的典型输出：思考段后跟一串空行再进正文
        assert_eq!(run(&["<think>盘算</think>\n\n\n\n译文"]), "译文");
    }

    #[test]
    fn drops_leading_whitespace_without_think() {
        assert_eq!(run(&["\n\n  译文"]), "译文");
    }

    #[test]
    fn drops_trailing_whitespace() {
        assert_eq!(run(&["译文\n\n\n"]), "译文");
    }

    #[test]
    fn keeps_paragraph_break_in_the_middle() {
        // 中间的段落间距要保留
        assert_eq!(run(&["第一段\n\n", "第二段"]), "第一段\n\n第二段");
    }

    #[test]
    fn whitespace_only_stream_yields_nothing() {
        assert_eq!(run(&["  \n\n  "]), "");
    }

    #[test]
    fn trailing_whitespace_across_chunk_boundary() {
        // 空白分两片到、之后再无正文：全丢
        assert_eq!(run(&["译文\n", "\n\n"]), "译文");
    }
}
