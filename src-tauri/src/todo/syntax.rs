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
    /// 引号屏蔽区间（含引号本身）。渲染层据此把 `"@张三"` 显示为 `@张三`：
    /// 引号是语法的一部分，不该出现在渲染态里，但也绝不能从文本中删掉。
    pub quoted: Vec<Span>,
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
        quoted,
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

// ---------- 序列化：结构 → 规范文本 ----------
//
// 这是解析的反方向，同样只有 Rust 一份实现。前端可以格式化用于**显示**的文案
// （`8月14日 18:00`），但任何要进入文档的文本都必须从这里产出。

/// 序列化单个标记为规范文本（含标记字符）。
///
/// `None` 表示「这个值不该被写进文本」，有两种情形：
/// - 默认值 `!once` / `^toast`：约定省略不写
/// - 当前行内语法无法安全表达的值：含空白或引号的标签、空标签、位宽非法的 ID、
///   `n == 0` 的周期、date 与 time 皆为 None 的时间表达式
///
/// 二者由 [`is_omitted_default`] 区分：前者应当删除已有标记，后者必须报错。
/// 绝不返回一个「能写但解析回来不一样」的字符串。
pub fn serialize_marker(value: &MarkerValue) -> Option<String> {
    match value {
        MarkerValue::Time(expr) => serialize_time(expr).map(|s| format!("@{s}")),
        MarkerValue::Repeat(r) => serialize_recurrence(r).map(|s| format!("!{s}")),
        MarkerValue::Tag(t) => serialize_tag(t).map(|s| format!("#{s}")),
        MarkerValue::Intensity(i) => serialize_intensity(*i).map(|s| format!("^{s}")),
        // 用 parse_id 校验而不是另写一份位宽规则，避免两处规则漂移
        MarkerValue::Id(id) => parse_id(id).ok().map(|id| format!("~{id}")),
    }
}

/// 该取值是否属于「约定省略不写」的默认值
fn is_omitted_default(value: &MarkerValue) -> bool {
    matches!(
        value,
        MarkerValue::Repeat(Recurrence::Once) | MarkerValue::Intensity(Intensity::Toast)
    )
}

fn serialize_time(expr: &TimeExpr) -> Option<String> {
    let date = match &expr.date {
        Some(DatePart::Absolute { year, month, day }) => {
            // 调用方可能构造出 2026-13-45 这种结构。写出去就成了解析不回来的
            // 文本，所以在这里拦住而不是交给磁盘。
            if *month == 0 || *month > 12 || *day == 0 || *day > days_in_month(*year, *month) {
                return None;
            }
            Some(format!("{year:04}-{month:02}-{day:02}"))
        }
        Some(DatePart::Today) => Some("today".to_string()),
        Some(DatePart::Tomorrow) => Some("tomorrow".to_string()),
        None => None,
    };

    let time = match expr.time {
        // 时刻非法时整个标记都不写。只丢掉时刻部分会把 @2026-08-14 25:00
        // 静默变成 @2026-08-14，那是改坏了用户的意图。
        Some((h, m)) if h > 23 || m > 59 => return None,
        Some((h, m)) => Some(format!("{h:02}:{m:02}")),
        None => None,
    };

    match (date, time) {
        (Some(d), Some(t)) => Some(format!("{d} {t}")),
        (Some(d), None) => Some(d),
        (None, Some(t)) => Some(t),
        // TimeExpr 的不变式是两者至少有一个，破了就是构造方的错，不写
        (None, None) => None,
    }
}

fn serialize_recurrence(r: &Recurrence) -> Option<String> {
    match r {
        Recurrence::Once => None, // 默认值，省略不写
        Recurrence::Daily => Some("daily".to_string()),
        Recurrence::Weekly => Some("weekly".to_string()),
        Recurrence::Monthly => Some("monthly".to_string()),
        Recurrence::Yearly => Some("yearly".to_string()),
        Recurrence::Weekdays => Some("weekdays".to_string()),
        Recurrence::EveryDays { n } if *n > 0 => Some(format!("every_{n}d")),
        Recurrence::EveryWeeks { n } if *n > 0 => Some(format!("every_{n}w")),
        // every_0d 解析不回来
        Recurrence::EveryDays { .. } | Recurrence::EveryWeeks { .. } => None,
    }
}

/// 标签目前无法表达空白：`#"我的 标签"` 会被分词器在引号处切断，
/// 解析回来是空标签。所以含空白的标签一律拒绝，而不是写一个坏的进去。
fn serialize_tag(tag: &str) -> Option<String> {
    if tag.is_empty() || tag.chars().any(|c| c.is_whitespace() || c == '"') {
        return None;
    }
    Some(tag.to_string())
}

fn serialize_intensity(i: Intensity) -> Option<String> {
    match i {
        Intensity::Toast => None, // 默认值，省略不写
        Intensity::Ring => Some("ring".to_string()),
        Intensity::Full => Some("full".to_string()),
    }
}

/// 把一个标记写回到某一行：替换已有同类标记，或追加到元数据区末尾。
///
/// 保证只改动元数据区——正文、缩进、行尾空白、以及其他标记全部逐字节不变。
///
/// 默认值（`!once` / `^toast`）的语义是「取消」：已有同类标记时删除它，
/// 本来就没有时什么都不做。
///
/// 标签可以重复，所以不做同类替换：同名标签已存在则原样返回，否则追加。
pub fn write_marker_to_line(line: &str, value: &MarkerValue) -> Result<String, String> {
    if line.trim().is_empty() {
        return Err("空行无法承载标记".to_string());
    }

    let parsed = parse(line).unwrap_or_else(|| parse_fragment(line));
    let kind = value.kind();

    let existing = if kind == MarkerKind::Tag {
        None
    } else {
        parsed
            .markers
            .iter()
            .find(|m| m.value.kind() == kind)
            .map(|m| m.span)
    };

    match (serialize_marker(value), existing) {
        // 写不出来又不是默认值：可见地失败，绝不写一个解析不回来的字符串
        (None, _) if !is_omitted_default(value) => {
            Err(format!("{kind:?} 的取值无法用行内语法表达，拒绝写入"))
        }
        (None, Some(span)) => Ok(remove_span(line, span)),
        (None, None) => Ok(line.to_string()),
        (Some(text), Some(span)) => Ok(format!(
            "{}{}{}",
            &line[..span.start],
            text,
            &line[span.end + 1..]
        )),
        (Some(text), None) => {
            if kind == MarkerKind::Tag && parsed.markers.iter().any(|m| m.value == *value) {
                return Ok(line.to_string()); // 同名标签已在，不重复追加
            }
            Ok(append_marker(line, &text))
        }
    }
}

/// 删除某个标记，连带它前面那一个分隔空格
fn remove_span(line: &str, span: Span) -> String {
    let mut start = span.start;
    if start > 0 && line.as_bytes()[start - 1] == b' ' {
        start -= 1;
    }
    format!("{}{}", &line[..start], &line[span.end + 1..])
}

/// 追加标记到元数据区末尾。
///
/// 插入点取最后一个非空白字节之后，而不是行尾——这样行尾原有的空白留在
/// 标记后面，仍然逐字节不变。用 `trim_end()` 直接拼接会吃掉它们。
fn append_marker(line: &str, text: &str) -> String {
    let insert_at = line.trim_end().len();
    format!("{} {}{}", &line[..insert_at], text, &line[insert_at..])
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

    #[test]
    fn test_quoted_ranges_are_exposed_for_rendering() {
        // 渲染层要把 `"@张三"` 显示成 `@张三`，得知道引号在哪；
        // 让它自己找引号等于在前端重写一遍分词器，已被否决。
        let line = r#"- [ ] 联系 "@张三" 确认"#;
        let result = parse(line).unwrap();
        assert_eq!(result.quoted.len(), 1);
        let q = result.quoted[0];
        assert_eq!(&line[q.start..=q.end], r#""@张三""#);
        assert!(result.markers.is_empty(), "引号内的 @ 不该成为标记");
    }

    #[test]
    fn test_unclosed_quote_exposes_no_range() {
        let result = parse(r#"- [ ] 他说 "没关系"#).unwrap();
        assert!(result.quoted.is_empty(), "未闭合引号不产生屏蔽区间");
    }

    #[test]
    fn test_json_shape_is_what_frontend_expects() {
        // 前端 decoration 直接吃这个 JSON。形状变了就是静默的渲染错误，
        // 所以把契约钉在测试里，而不是靠两边各自记得。
        let line = "- [ ] 交周报 @2026-08-14 18:00 !every_3d #工作 ^ring ~a3f9";
        let parsed = parse(line).unwrap();
        let json = serde_json::to_value(&parsed).unwrap();

        assert_eq!(json["checked"], serde_json::json!(false));

        let by_kind = |k: &str| -> serde_json::Value {
            json["markers"]
                .as_array()
                .unwrap()
                .iter()
                .find(|m| m["value"]["kind"] == k)
                .unwrap()["value"]
                .clone()
        };

        // 时间：date 是内部带标签的对象，time 是 [时, 分] 数组
        assert_eq!(
            by_kind("time")["value"],
            serde_json::json!({
                "date": { "kind": "absolute", "year": 2026, "month": 8, "day": 14 },
                "time": [18, 0]
            })
        );

        // 重复：单个对象（不是数组），every_Nd 的数字在 `n` 上
        assert_eq!(
            by_kind("repeat")["value"],
            serde_json::json!({ "kind": "every_days", "n": 3 })
        );

        // 标签：裸字符串
        assert_eq!(by_kind("tag")["value"], serde_json::json!("工作"));

        // 强度：裸字符串（不是 { kind: ... }）
        assert_eq!(by_kind("intensity")["value"], serde_json::json!("ring"));

        // ID：裸字符串
        assert_eq!(by_kind("id")["value"], serde_json::json!("a3f9"));
    }

    // ---------- 0.3 序列化往返一致 ----------

    fn time(date: Option<DatePart>, t: Option<(u32, u32)>) -> MarkerValue {
        MarkerValue::Time(TimeExpr { date, time: t })
    }

    fn abs(year: u32, month: u32, day: u32) -> Option<DatePart> {
        Some(DatePart::Absolute { year, month, day })
    }

    /// 序列化 → 放进一行 → 解析回来，必须得到等价结构。
    /// 用 parse（真实的待办行形态）而不是 parse_fragment，因为写回的对象是文档里的行。
    fn assert_round_trip(value: MarkerValue) {
        let text = serialize_marker(&value)
            .unwrap_or_else(|| panic!("{value:?} 应当可序列化"));
        let line = format!("- [ ] 正文 {text}");
        let parsed = parse(&line).unwrap_or_else(|| panic!("{line:?} 应当仍是待办行"));
        assert_eq!(
            parsed.markers.len(),
            1,
            "{line:?} 应当只解析出一个标记，实到 {:?}（降级 {:?}）",
            parsed.markers,
            parsed.degraded
        );
        assert_eq!(parsed.markers[0].value, value, "往返后结构变了：{line:?}");
        assert_eq!(
            &line[parsed.markers[0].span.start..=parsed.markers[0].span.end],
            text,
            "span 未覆盖完整标记文本"
        );
    }

    #[test]
    fn test_round_trip_time() {
        assert_round_trip(time(abs(2026, 8, 14), Some((18, 0))));
        assert_round_trip(time(abs(2026, 8, 14), None));
        assert_round_trip(time(None, Some((18, 0))));
        assert_round_trip(time(None, Some((0, 0))));
        assert_round_trip(time(None, Some((23, 59))));
        assert_round_trip(time(Some(DatePart::Today), None));
        assert_round_trip(time(Some(DatePart::Today), Some((20, 0))));
        assert_round_trip(time(Some(DatePart::Tomorrow), None));
        assert_round_trip(time(Some(DatePart::Tomorrow), Some((9, 5))));
        // 闰年 2 月 29 日
        assert_round_trip(time(abs(2028, 2, 29), None));
    }

    #[test]
    fn test_round_trip_time_pads_zeroes() {
        // 补零是规范格式的一部分：不补零解析不回来
        let text = serialize_marker(&time(abs(2026, 1, 2), Some((9, 5)))).unwrap();
        assert_eq!(text, "@2026-01-02 09:05");
    }

    #[test]
    fn test_round_trip_repeat() {
        for r in [
            Recurrence::Daily,
            Recurrence::Weekly,
            Recurrence::Monthly,
            Recurrence::Yearly,
            Recurrence::Weekdays,
            Recurrence::EveryDays { n: 3 },
            Recurrence::EveryDays { n: 1 },
            Recurrence::EveryWeeks { n: 2 },
        ] {
            assert_round_trip(MarkerValue::Repeat(r));
        }
    }

    #[test]
    fn test_round_trip_tag_and_intensity_and_id() {
        assert_round_trip(MarkerValue::Tag("工作".to_string()));
        assert_round_trip(MarkerValue::Tag("work".to_string()));
        assert_round_trip(MarkerValue::Intensity(Intensity::Ring));
        assert_round_trip(MarkerValue::Intensity(Intensity::Full));
        assert_round_trip(MarkerValue::Id("a3f9".to_string()));
        assert_round_trip(MarkerValue::Id("deadbeef".to_string()));
    }

    #[test]
    fn test_defaults_are_omitted() {
        // !once / ^toast 约定不写
        assert_eq!(serialize_marker(&MarkerValue::Repeat(Recurrence::Once)), None);
        assert_eq!(
            serialize_marker(&MarkerValue::Intensity(Intensity::Toast)),
            None
        );
    }

    #[test]
    fn test_unrepresentable_values_refuse_to_serialize() {
        // 全部是「写出去就解析不回来」的值，一律不写
        let bad = [
            time(None, None),                       // 破了 TimeExpr 不变式
            time(abs(2026, 13, 1), None),           // 月份越界
            time(abs(2026, 2, 30), None),           // 2 月没有 30 日
            time(abs(2027, 2, 29), None),           // 非闰年
            time(abs(2026, 8, 14), Some((25, 0))),  // 小时越界
            time(abs(2026, 8, 14), Some((12, 60))), // 分钟越界
            MarkerValue::Repeat(Recurrence::EveryDays { n: 0 }),
            MarkerValue::Repeat(Recurrence::EveryWeeks { n: 0 }),
            MarkerValue::Tag(String::new()),
            MarkerValue::Tag("我的 标签".to_string()),
            MarkerValue::Tag("带\"引号".to_string()),
            MarkerValue::Id("xy".to_string()),        // 太短
            MarkerValue::Id("0123456789".to_string()), // 太长
            MarkerValue::Id("zzzz".to_string()),      // 非十六进制
        ];
        for value in bad {
            assert_eq!(
                serialize_marker(&value),
                None,
                "{value:?} 不该被序列化成文本"
            );
        }
    }

    #[test]
    fn test_id_is_normalized_to_lowercase() {
        // 大写十六进制合法但非规范形态，写回时统一小写
        assert_eq!(
            serialize_marker(&MarkerValue::Id("A3F9".to_string())).as_deref(),
            Some("~a3f9")
        );
    }

    // ---------- 0.4 写回只改元数据区 ----------

    #[test]
    fn test_write_appends_when_absent() {
        let out = write_marker_to_line(
            "- [ ] 交周报",
            &time(abs(2026, 8, 14), Some((18, 0))),
        )
        .unwrap();
        assert_eq!(out, "- [ ] 交周报 @2026-08-14 18:00");
    }

    #[test]
    fn test_write_replaces_same_kind_in_place() {
        // 时间标记横跨两个 token，替换成只有时刻的形态后其余部分必须逐字节不变
        let line = "- [ ] 交周报 @2026-08-14 18:00 !daily ^ring ~a3f9";
        let out = write_marker_to_line(line, &time(None, Some((9, 30)))).unwrap();
        assert_eq!(out, "- [ ] 交周报 @09:30 !daily ^ring ~a3f9");
    }

    #[test]
    fn test_write_preserves_indent_and_body_bytes() {
        // 缩进、正文里的 @ 与 #、以及其他标记都不该被碰
        let line = "  - [x] 联系 zhang@corp.com 见 #C-3 号楼 !weekly";
        let out = write_marker_to_line(line, &MarkerValue::Intensity(Intensity::Full)).unwrap();
        assert_eq!(out, format!("{line} ^full"));
    }

    #[test]
    fn test_write_preserves_trailing_whitespace() {
        // 追加点取最后一个非空白字节之后，行尾原有空白留在标记后面
        let out = write_marker_to_line("- [ ] 交周报   ", &MarkerValue::Tag("工作".to_string()))
            .unwrap();
        assert_eq!(out, "- [ ] 交周报 #工作   ");
    }

    #[test]
    fn test_write_preserves_crlf_carriage_return() {
        // CRLF 文件按 \n 切行后每行尾部带 \r，它必须留在行尾而不是被标记顶开
        let out =
            write_marker_to_line("- [ ] 交周报\r", &MarkerValue::Repeat(Recurrence::Daily)).unwrap();
        assert_eq!(out, "- [ ] 交周报 !daily\r");
    }

    #[test]
    fn test_write_default_removes_existing_marker() {
        // ^toast / !once 的语义是「取消」：删掉已有标记连带它前面那个分隔空格
        let out = write_marker_to_line(
            "- [ ] 交周报 @2026-08-14 ^ring",
            &MarkerValue::Intensity(Intensity::Toast),
        )
        .unwrap();
        assert_eq!(out, "- [ ] 交周报 @2026-08-14");

        let out = write_marker_to_line(
            "- [ ] 交周报 !daily ~a3f9",
            &MarkerValue::Repeat(Recurrence::Once),
        )
        .unwrap();
        assert_eq!(out, "- [ ] 交周报 ~a3f9");
    }

    #[test]
    fn test_write_default_on_absent_marker_is_noop() {
        let line = "- [ ] 交周报 @2026-08-14";
        assert_eq!(
            write_marker_to_line(line, &MarkerValue::Intensity(Intensity::Toast)).unwrap(),
            line
        );
        assert_eq!(
            write_marker_to_line(line, &MarkerValue::Repeat(Recurrence::Once)).unwrap(),
            line
        );
    }

    #[test]
    fn test_write_tag_appends_and_dedups() {
        // 标签可以多个，所以不做同类替换
        let out =
            write_marker_to_line("- [ ] 交周报 #工作", &MarkerValue::Tag("紧急".to_string()))
                .unwrap();
        assert_eq!(out, "- [ ] 交周报 #工作 #紧急");
        // 同名已在则原样返回
        assert_eq!(
            write_marker_to_line(&out, &MarkerValue::Tag("工作".to_string())).unwrap(),
            out
        );
    }

    #[test]
    fn test_write_works_on_fragment_without_prefix() {
        // 速记条的真实输入形态：没有 GFM 前缀，写回同样要成立
        let out = write_marker_to_line("买牛奶 @2026-08-15 18:00", &time(None, Some((7, 0))))
            .unwrap();
        assert_eq!(out, "买牛奶 @07:00");
    }

    #[test]
    fn test_write_refuses_unrepresentable_value() {
        // 宁可可见地失败，也不写一个解析不回来的字符串
        let line = "- [ ] 交周报 @2026-08-14";
        assert!(write_marker_to_line(line, &MarkerValue::Tag("我的 标签".to_string())).is_err());
        assert!(write_marker_to_line(line, &time(abs(2026, 2, 30), None)).is_err());
        assert!(write_marker_to_line(line, &MarkerValue::Id("zz".to_string())).is_err());
    }

    #[test]
    fn test_write_refuses_empty_line() {
        assert!(write_marker_to_line("", &MarkerValue::Repeat(Recurrence::Daily)).is_err());
        assert!(write_marker_to_line("   ", &MarkerValue::Repeat(Recurrence::Daily)).is_err());
    }

    #[test]
    fn test_write_then_parse_round_trip() {
        // 写回的结果必须能被解析器读回同一个值（写回 × 解析的联合不变式）
        let line = "- [ ] 交周报";
        let value = time(Some(DatePart::Tomorrow), Some((9, 0)));
        let out = write_marker_to_line(line, &value).unwrap();
        let parsed = parse(&out).unwrap();
        assert_eq!(parsed.markers.len(), 1);
        assert_eq!(parsed.markers[0].value, value);
        assert_eq!(&out[parsed.content.start..=parsed.content.end], "交周报");
    }
}
