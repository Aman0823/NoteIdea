//! 待办行内联语法解析器（D1, D3, D4, D9）
//!
//! 识别 GFM 复选框前缀 + 行尾元数据区的五种标记：
//! - `@时间` 提醒时间（绝对日期/today/tomorrow + 可选时分）
//! - `!重复` 重复规则（once/daily/weekly/monthly/yearly/weekdays/every_Nd/every_Nw）
//! - `#标签` 标签（可重复）
//! - `^强度` 提醒强度（toast/ring/full）
//! - `~id` 待办 ID（4-8 位十六进制）
//!
//! 解析策略（D9 + design.md）：
//! 1. 先做引号配对，标记引号内区间为「不可解析」（未闭合引号视为普通字符）
//! 2. 按空格分词（跳过引号内区间）
//! 3. 从右向左扫描 token，遇首个非法 token 立即停止
//! 4. 非法值降级为正文，记录 `Degraded` 供 UI 提示
//!
//! 对应 spec：todo/syntax

use serde::{Deserialize, Serialize};

/// 字节区间（闭区间 [start, end]，用于标记 token 在原始字符串中的位置）
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Span {
    pub start: usize,
    pub end: usize,
}

/// 标记种类
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MarkerKind {
    Time,
    Repeat,
    Tag,
    Intensity,
    Id,
}

/// 日期部分（未求值）
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum DatePart {
    /// 绝对日期 YYYY-MM-DD
    Absolute { year: u32, month: u32, day: u32 },
    /// today（求值时需当前日期）
    Today,
    /// tomorrow（求值时需当前日期）
    Tomorrow,
}

/// 时间表达式（未求值，保留原始形态供后续 evaluate 函数处理）
///
/// 不变式：date 和 time 至少有一个是 Some
/// - `@18:00` → date=None, time=Some((18,0))
/// - `@2026-08-14` → date=Some(Absolute), time=None
/// - `@today 20:00` → date=Some(Today), time=Some((20,0))
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TimeExpr {
    pub date: Option<DatePart>,
    pub time: Option<(u32, u32)>, // (hour, minute)
}

/// 重复规则
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Recurrence {
    Once,
    Daily,
    Weekly,
    Monthly,
    Yearly,
    Weekdays,
    /// every_3d / every_7d
    EveryDays { n: u32 },
    /// every_2w / every_4w
    EveryWeeks { n: u32 },
}

/// 提醒强度
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Intensity {
    Toast,
    Ring,
    Full,
}

/// 标记取值（邻接标签 serde：kind 是枚举判别式，value 是内容）
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "value", rename_all = "snake_case")]
pub enum MarkerValue {
    Time(TimeExpr),
    Repeat(Recurrence),
    Tag(String),
    Intensity(Intensity),
    Id(String),
}

impl MarkerValue {
    #[allow(dead_code)] // 用于后续任务的 UI 渲染
    pub fn kind(&self) -> MarkerKind {
        match self {
            Self::Time(_) => MarkerKind::Time,
            Self::Repeat(_) => MarkerKind::Repeat,
            Self::Tag(_) => MarkerKind::Tag,
            Self::Intensity(_) => MarkerKind::Intensity,
            Self::Id(_) => MarkerKind::Id,
        }
    }
}

/// 已识别标记（取值 + 位置）
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Marker {
    pub value: MarkerValue,
    pub span: Span,
}

/// 降级 token（非法值，记录疑似意图种类 + 位置供 UI 提示）
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Degraded {
    pub suspected: MarkerKind,
    pub span: Span,
}

/// 解析结果（None = 不是待办行）
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TodoLine {
    /// 是否已勾选
    pub checked: bool,
    /// 正文区间（不含前缀、不含元数据）
    pub content: Span,
    /// 已识别标记（右向左扫描顺序，即从后往前）
    pub markers: Vec<Marker>,
    /// 降级 token（记录位置供 UI 提示）
    pub degraded: Vec<Degraded>,
}

impl TodoLine {
    /// 根据字节偏移量查找所在的 marker（用于光标定位）
    #[allow(dead_code)] // 用于后续任务的输入辅助弹层
    pub fn marker_at(&self, offset: usize) -> Option<&Marker> {
        self.markers
            .iter()
            .find(|m| offset >= m.span.start && offset <= m.span.end)
    }
}

/// 解析待办行
///
/// 返回 None 表示不是待办行（没有 GFM 复选框前缀）
pub fn parse(line: &str) -> Option<TodoLine> {
    let (checked, content_start) = todo_prefix(line)?;
    Some(scan(line, checked, content_start))
}

/// 扫描元数据区。`parse` 与 `parse_fragment` 的公共实现——
/// 二者只在「是否要求 GFM 前缀」上不同，扫描规则完全共享。
fn scan(line: &str, checked: bool, content_start: usize) -> TodoLine {
    // 引号配对：标记引号内区间为「不可解析」
    let quoted = quoted_ranges(line);

    // 分词：按空格切分，跳过引号内区间
    let tokens = tokenize(line, content_start, &quoted);

    // 从右向左扫描元数据区
    let mut markers = Vec::new();
    let mut degraded = Vec::new();
    let mut seen_kinds = std::collections::HashSet::new();
    let mut consumed = std::collections::HashSet::new(); // 记录已被 Case B 消费的 token 索引
    let mut metadata_end = tokens.len();

    for i in (0..tokens.len()).rev() {
        // 跳过已被 Case B 消费的 token
        if consumed.contains(&i) {
            continue;
        }

        let tok = &tokens[i];

        // Case A: token 首字符是标记字符（未被引号屏蔽）
        if !is_in_quoted(tok.start, &quoted) {
            let first_ch = line.as_bytes()[tok.start];
            if let Some(kind) = marker_char_to_kind(first_ch) {
                match parse_marker(line, tok, kind) {
                    Ok(value) => {
                        // 检查重复：除了 tag，其他种类只能出现一次
                        if kind != MarkerKind::Tag && seen_kinds.contains(&kind) {
                            degraded.push(Degraded {
                                suspected: kind,
                                span: *tok,
                            });
                            metadata_end = i + 1;
                            break;
                        }
                        seen_kinds.insert(kind);
                        markers.push(Marker {
                            value,
                            span: *tok,
                        });
                        continue;
                    }
                    Err(()) => {
                        // 非法值：记录降级并终止扫描
                        degraded.push(Degraded {
                            suspected: kind,
                            span: *tok,
                        });
                        metadata_end = i + 1;
                        break;
                    }
                }
            }
        }

        // Case B: 检查是否是两个 token 拼成的时间表达式（@2026-08-14 18:00）
        if i > 0 {
            let prev = &tokens[i - 1];
            if !is_in_quoted(prev.start, &quoted)
                && line.as_bytes()[prev.start] == b'@'
            {
                let glued_start = prev.start;
                let glued_end = tok.end;
                let glued = &line[glued_start..=glued_end];
                if let Ok(value) = parse_two_token_time(glued) {
                    if seen_kinds.contains(&MarkerKind::Time) {
                        degraded.push(Degraded {
                            suspected: MarkerKind::Time,
                            span: Span {
                                start: glued_start,
                                end: glued_end,
                            },
                        });
                        metadata_end = i - 1;
                        break;
                    }
                    seen_kinds.insert(MarkerKind::Time);
                    markers.push(Marker {
                        value,
                        span: Span {
                            start: glued_start,
                            end: glued_end,
                        },
                    });
                    // 标记前一个 token 为已消费，避免重复处理
                    consumed.insert(i - 1);
                    continue;
                }
            }
        }

        // 不是标记：终止扫描
        metadata_end = i + 1;
        break;
    }

    // 正文区间：从前缀结束到元数据区开始
    let content_end = if metadata_end == tokens.len() {
        // 没有识别到元数据：正文到最后一个 token 结尾
        if tokens.is_empty() {
            content_start.saturating_sub(1)
        } else {
            tokens.last().unwrap().end
        }
    } else if metadata_end == 0 {
        // 所有 token 都是元数据：正文只有前缀
        content_start.saturating_sub(1)
    } else {
        // 元数据区从 metadata_end 开始：正文到前一个 token 结尾
        tokens[metadata_end - 1].end
    };

    TodoLine {
        checked,
        content: Span {
            start: content_start,
            end: content_end,
        },
        markers,
        degraded,
    }
}

/// 解析裸文本片段：不要求 GFM 复选框前缀，其余规则与 `parse` 完全一致。
///
/// 速记条里用户敲的是 `买牛奶 @2026-08-15 18:00`，没有 `- [ ] ` 前缀；
/// 用 `parse` 会一律得到 None，调用方会以为「这行没有任何标记」。
/// 将来编辑器里在普通段落敲 `@` 也是同样情形。
///
/// 复用同一套扫描逻辑，语法规则仍是唯一一份（D1）。
/// `checked` 恒为 false（没有复选框可言）。
pub fn parse_fragment(text: &str) -> TodoLine {
    scan(text, false, 0)
}

/// 识别 GFM 复选框前缀（容忍前导空格、`*` / `-` 列表符）
///
/// 返回 (是否已勾选, 正文起始位置)
fn todo_prefix(line: &str) -> Option<(bool, usize)> {
    let trimmed = line.trim_start();
    let leading_ws = line.len() - trimmed.len();

    // 列表符 `- ` 或 `* `
    let after_bullet = if trimmed.starts_with("- ") || trimmed.starts_with("* ") {
        trimmed[2..].trim_start()
    } else {
        trimmed
    };

    // 复选框 `[ ]` 或 `[x]` / `[X]`
    if after_bullet.starts_with("[ ] ") {
        let pos = leading_ws + (trimmed.len() - after_bullet.len()) + 4;
        return Some((false, pos));
    }
    if after_bullet.starts_with("[x] ") || after_bullet.starts_with("[X] ") {
        let pos = leading_ws + (trimmed.len() - after_bullet.len()) + 4;
        return Some((true, pos));
    }

    None
}

/// 引号配对：返回所有引号内区间（未闭合的引号视为普通字符，不产生屏蔽区间）
fn quoted_ranges(line: &str) -> Vec<Span> {
    let mut ranges = Vec::new();
    let mut in_quote = false;
    let mut quote_start = 0;

    for (i, ch) in line.bytes().enumerate() {
        if ch == b'"' {
            if in_quote {
                // 闭合：记录区间（含引号本身）
                ranges.push(Span {
                    start: quote_start,
                    end: i,
                });
                in_quote = false;
            } else {
                quote_start = i;
                in_quote = true;
            }
        }
    }

    // 未闭合的引号不产生屏蔽区间
    ranges
}

/// 分词：按空格切分，跳过引号内区间
fn tokenize(line: &str, start: usize, quoted: &[Span]) -> Vec<Span> {
    let mut tokens = Vec::new();
    let mut tok_start = None;

    for i in start..line.len() {
        if is_in_quoted(i, quoted) {
            // 引号内：跳过整个区间
            if let Some(_q) = quoted.iter().find(|q| i >= q.start && i <= q.end) {
                if tok_start.is_some() {
                    tokens.push(Span {
                        start: tok_start.unwrap(),
                        end: i - 1,
                    });
                    tok_start = None;
                }
                // 跳到引号区间结束
                continue;
            }
        }

        let ch = line.as_bytes()[i];
        if ch.is_ascii_whitespace() {
            if let Some(s) = tok_start {
                tokens.push(Span { start: s, end: i - 1 });
                tok_start = None;
            }
        } else if tok_start.is_none() {
            tok_start = Some(i);
        }
    }

    if let Some(s) = tok_start {
        tokens.push(Span {
            start: s,
            end: line.len() - 1,
        });
    }

    tokens
}

/// 判断字节位置是否在引号内
fn is_in_quoted(pos: usize, quoted: &[Span]) -> bool {
    quoted.iter().any(|q| pos >= q.start && pos <= q.end)
}

/// 标记字符 → 种类
fn marker_char_to_kind(ch: u8) -> Option<MarkerKind> {
    match ch {
        b'@' => Some(MarkerKind::Time),
        b'!' => Some(MarkerKind::Repeat),
        b'#' => Some(MarkerKind::Tag),
        b'^' => Some(MarkerKind::Intensity),
        b'~' => Some(MarkerKind::Id),
        _ => None,
    }
}

/// 解析单个标记（token 首字符已确认是标记字符）
///
/// 返回 Err(()) 表示非法值
fn parse_marker(line: &str, tok: &Span, kind: MarkerKind) -> Result<MarkerValue, ()> {
    let text = &line[tok.start..=tok.end];
    let value_str = &text[1..]; // 跳过标记字符

    // 如果紧跟引号，去掉引号作为值内容
    let value_str = if value_str.starts_with('"') && value_str.ends_with('"') && value_str.len() >= 2 {
        &value_str[1..value_str.len() - 1]
    } else {
        value_str
    };

    match kind {
        MarkerKind::Time => parse_time_expr(value_str).map(MarkerValue::Time),
        MarkerKind::Repeat => parse_recurrence(value_str).map(MarkerValue::Repeat),
        MarkerKind::Tag => Ok(MarkerValue::Tag(value_str.to_string())),
        MarkerKind::Intensity => parse_intensity(value_str).map(MarkerValue::Intensity),
        MarkerKind::Id => parse_id(value_str).map(MarkerValue::Id),
    }
}

/// 解析两个 token 拼成的时间表达式（@2026-08-14 18:00）
fn parse_two_token_time(glued: &str) -> Result<MarkerValue, ()> {
    if !glued.starts_with('@') {
        return Err(());
    }
    parse_time_expr(&glued[1..]).map(MarkerValue::Time)
}

/// 解析时间表达式（不求值，返回未展开形态）
///
/// 支持格式：
/// - `18:00` → time only
/// - `2026-08-14` → date only
/// - `2026-08-14 18:00` → date + time
/// - `today` / `today 20:00`
/// - `tomorrow` / `tomorrow 09:00`
fn parse_time_expr(s: &str) -> Result<TimeExpr, ()> {
    let parts: Vec<&str> = s.split_ascii_whitespace().collect();
    if parts.is_empty() {
        return Err(());
    }

    let mut date = None;
    let mut time = None;

    for part in parts {
        // 尝试解析为日期
        if part == "today" {
            date = Some(DatePart::Today);
            continue;
        }
        if part == "tomorrow" {
            date = Some(DatePart::Tomorrow);
            continue;
        }
        if part.contains('-') {
            // YYYY-MM-DD
            if let Some(d) = parse_absolute_date(part) {
                date = Some(d);
                continue;
            }
        }
        // 尝试解析为时间
        if part.contains(':') {
            if let Some(t) = parse_time(part) {
                time = Some(t);
                continue;
            }
        }

        // 既不是日期也不是时间：非法
        return Err(());
    }

    // 至少有一个
    if date.is_none() && time.is_none() {
        return Err(());
    }

    Ok(TimeExpr { date, time })
}

/// 解析绝对日期 YYYY-MM-DD
fn parse_absolute_date(s: &str) -> Option<DatePart> {
    let parts: Vec<&str> = s.split('-').collect();
    if parts.len() != 3 {
        return None;
    }
    let year: u32 = parts[0].parse().ok()?;
    let month: u32 = parts[1].parse().ok()?;
    let day: u32 = parts[2].parse().ok()?;

    // 基本校验
    if month == 0 || month > 12 || day == 0 || day > 31 {
        return None;
    }
    // 更严格的天数校验（考虑闰年）
    if day > days_in_month(year, month) {
        return None;
    }

    Some(DatePart::Absolute { year, month, day })
}

/// 解析时间 HH:MM
fn parse_time(s: &str) -> Option<(u32, u32)> {
    let parts: Vec<&str> = s.split(':').collect();
    if parts.len() != 2 {
        return None;
    }
    let hour: u32 = parts[0].parse().ok()?;
    let minute: u32 = parts[1].parse().ok()?;

    if hour > 23 || minute > 59 {
        return None;
    }

    Some((hour, minute))
}

/// 某年某月的天数（用于日期校验，group 2 时间求值也会复用）
pub fn days_in_month(year: u32, month: u32) -> u32 {
    match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 => {
            if is_leap(year) {
                29
            } else {
                28
            }
        }
        _ => 0,
    }
}

/// 是否闰年
pub fn is_leap(year: u32) -> bool {
    (year % 4 == 0 && year % 100 != 0) || (year % 400 == 0)
}

/// 解析重复规则
fn parse_recurrence(s: &str) -> Result<Recurrence, ()> {
    match s {
        "once" => Ok(Recurrence::Once),
        "daily" => Ok(Recurrence::Daily),
        "weekly" => Ok(Recurrence::Weekly),
        "monthly" => Ok(Recurrence::Monthly),
        "yearly" => Ok(Recurrence::Yearly),
        "weekdays" => Ok(Recurrence::Weekdays),
        _ => {
            // every_3d / every_2w
            if let Some(rest) = s.strip_prefix("every_") {
                if let Some(num_str) = rest.strip_suffix('d') {
                    let n: u32 = num_str.parse().map_err(|_| ())?;
                    if n > 0 {
                        return Ok(Recurrence::EveryDays { n });
                    }
                }
                if let Some(num_str) = rest.strip_suffix('w') {
                    let n: u32 = num_str.parse().map_err(|_| ())?;
                    if n > 0 {
                        return Ok(Recurrence::EveryWeeks { n });
                    }
                }
            }
            Err(())
        }
    }
}

/// 解析提醒强度
fn parse_intensity(s: &str) -> Result<Intensity, ()> {
    match s {
        "toast" => Ok(Intensity::Toast),
        "ring" => Ok(Intensity::Ring),
        "full" => Ok(Intensity::Full),
        _ => Err(()),
    }
}

/// 解析待办 ID（4-8 位十六进制）
fn parse_id(s: &str) -> Result<String, ()> {
    if s.len() < 4 || s.len() > 8 {
        return Err(());
    }
    if !s.chars().all(|c| c.is_ascii_hexdigit()) {
        return Err(());
    }
    Ok(s.to_lowercase())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_not_a_todo() {
        assert!(parse("普通段落").is_none());
        assert!(parse("# 标题").is_none());
    }

    #[test]
    fn test_basic_todo() {
        let line = "- [ ] 买菜";
        let result = parse(line).unwrap();
        assert!(!result.checked);
        assert_eq!(&line[result.content.start..=result.content.end], "买菜");
        assert!(result.markers.is_empty());
    }

    #[test]
    fn test_checked_todo() {
        let line = "- [x] 已完成";
        let result = parse(line).unwrap();
        assert!(result.checked);
    }

    #[test]
    fn test_email_not_mistaken_as_time() {
        let line = "- [ ] 联系 zhang@corp.com 确认需求";
        let result = parse(line).unwrap();
        assert!(result.markers.is_empty());
        assert_eq!(
            &line[result.content.start..=result.content.end],
            "联系 zhang@corp.com 确认需求"
        );
    }

    #[test]
    fn test_metadata_after_body_stops_at_first_non_marker() {
        let line = "- [ ] 交周报 @2026-08-14 记得带U盘";
        let result = parse(line).unwrap();
        // "记得带U盘" 不是标记，扫描在 "记得" 处停止
        assert!(result.markers.is_empty());
        assert!(result.degraded.is_empty());
    }

    #[test]
    fn test_quoted_literal() {
        let line = r#"- [ ] 转发给 "@张三" #urgent"#;
        let result = parse(line).unwrap();
        // "@张三" 被引号包裹，是字面量；#urgent 是标记
        assert_eq!(result.markers.len(), 1);
        match &result.markers[0].value {
            MarkerValue::Tag(t) => assert_eq!(t, "urgent"),
            _ => panic!("expected tag"),
        }
    }

    #[test]
    fn test_unclosed_quote_ignored() {
        let line = r#"- [ ] 记录"未闭合 @18:00"#;
        let result = parse(line).unwrap();
        // 未闭合引号视为普通字符，不屏蔽 @18:00
        assert_eq!(result.markers.len(), 1);
        match &result.markers[0].value {
            MarkerValue::Time(_) => {}
            _ => panic!("expected time"),
        }
    }

    #[test]
    fn test_invalid_time_degrades() {
        let line = "- [ ] 任务 @无效时间";
        let result = parse(line).unwrap();
        assert!(result.markers.is_empty());
        assert_eq!(result.degraded.len(), 1);
        assert_eq!(result.degraded[0].suspected, MarkerKind::Time);
    }

    #[test]
    fn test_duplicate_non_tag_marker_degrades() {
        let line = "- [ ] 任务 @18:00 @19:00";
        let result = parse(line).unwrap();
        // 第一个 @18:00 识别，第二个降级
        assert_eq!(result.markers.len(), 1);
        assert_eq!(result.degraded.len(), 1);
    }

    #[test]
    fn test_multiple_tags_allowed() {
        let line = "- [ ] 任务 #work #urgent #p1";
        let result = parse(line).unwrap();
        assert_eq!(result.markers.len(), 3);
        for m in &result.markers {
            assert!(matches!(m.value, MarkerValue::Tag(_)));
        }
    }

    #[test]
    fn test_fragment_without_prefix() {
        // 速记条的真实输入形态：没有 GFM 前缀
        let text = "买牛奶 @2026-08-15 18:00";
        let result = parse_fragment(text);
        assert_eq!(result.markers.len(), 1, "无前缀也要认出时间标记");
        assert!(matches!(result.markers[0].value, MarkerValue::Time(_)));
        // parse 对同样的输入返回 None
        assert!(parse(text).is_none());
    }

    #[test]
    fn test_fragment_time_span_locates_existing_marker() {
        // 时间选择器要靠这个 span 做「覆盖」而非「追加」
        let text = "买牛奶 @2026-08-15 18:00";
        let result = parse_fragment(text);
        let span = result.markers[0].span;
        assert_eq!(&text[span.start..=span.end], "@2026-08-15 18:00");
    }

    #[test]
    fn test_fragment_halfdone_time_in_degraded() {
        // 只敲了 @ 还没选时间：落在 degraded，前端同样要能定位并覆盖
        let text = "买牛奶 @";
        let result = parse_fragment(text);
        assert!(result.markers.is_empty());
        assert_eq!(result.degraded.len(), 1);
        assert_eq!(result.degraded[0].suspected, MarkerKind::Time);
        let span = result.degraded[0].span;
        assert_eq!(&text[span.start..=span.end], "@");
    }

    #[test]
    fn test_fragment_plain_text_has_no_marker() {
        let result = parse_fragment("买牛奶");
        assert!(result.markers.is_empty());
        assert!(result.degraded.is_empty(), "纯文本不该产生警告");
    }

    #[test]
    fn test_two_token_time() {
        let line = "- [ ] 会议 @2026-08-14 18:00";
        let result = parse(line).unwrap();
        assert_eq!(result.markers.len(), 1);
        match &result.markers[0].value {
            MarkerValue::Time(t) => {
                assert!(matches!(t.date, Some(DatePart::Absolute { year: 2026, month: 8, day: 14 })));
                assert_eq!(t.time, Some((18, 0)));
            }
            _ => panic!("expected time"),
        }
    }
}
