//! 行内语法解析：GFM 复选框 + 行尾元数据标记（D1, D3, D4, D9）
//!
//! 解析策略（D9, design.md）：
//! 1. 识别 GFM 待办前缀（`- [ ]` / `- [x]`，容忍缩进与 `*` 号）
//! 2. 引号配对（左向右），未闭合引号视为普通字符
//! 3. 按空白分词（ASCII 空格/制表符），引号内区间不分割
//! 4. 行尾元数据区右向左扫描，遇首个非法 token 停止
//!
//! 标记种类：`@时间 !重复 #标签 ^强度 ~id`
//! 时间求值独立（D4），解析器只返回未求值的表达式结构。

use serde::{Deserialize, Serialize};

/// 字节区间（UTF-8 偏移）
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
    Absolute { year: u32, month: u32, day: u32 },
    Today,
    Tomorrow,
}

/// 时间表达式（未求值）
///
/// 不变量：date 与 time 至少有一个是 Some。
/// 示例：`@18:00` → date=None, time=Some((18,0))
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TimeExpr {
    pub date: Option<DatePart>,
    pub time: Option<(u32, u32)>, // (小时, 分钟)
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
    EveryDays { n: u32 },
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

/// 标记值（邻接标签 serde）
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

/// 已识别标记
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Marker {
    pub value: MarkerValue,
    pub span: Span,
}

/// 降级 token（疑似标记但取值非法）
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Degraded {
    pub suspected: MarkerKind,
    pub span: Span,
}

/// 解析结果
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TodoLine {
    pub checked: bool,
    pub content: Span,          // 正文区间（前缀之后、元数据区之前）
    pub markers: Vec<Marker>,   // 已识别标记（左到右顺序）
    pub degraded: Vec<Degraded>, // 降级 token（左到右顺序）
}

impl TodoLine {
    /// 查找包含给定字节偏移的标记
    pub fn marker_at(&self, offset: usize) -> Option<&Marker> {
        self.markers
            .iter()
            .find(|m| m.span.start <= offset && offset < m.span.end)
    }
}

/// 解析一行，返回 None 表示不是待办行
pub fn parse(line: &str) -> Option<TodoLine> {
    let (checked, content_start) = todo_prefix(line)?;

    // 引号配对（左向右），建立屏蔽区间
    let quoted = quoted_ranges(line);

    // 分词（引号内不分割）
    let tokens = tokenize(line, content_start, &quoted);

    // 右向左扫描元数据区
    let (markers, degraded, content_end) = scan_metadata(line, &tokens, &quoted);

    Some(TodoLine {
        checked,
        content: Span {
            start: content_start,
            end: content_end,
        },
        markers,
        degraded,
    })
}

/// 识别 GFM 待办前缀，返回 (是否勾选, 正文起始位置)
fn todo_prefix(line: &str) -> Option<(bool, usize)> {
    let trimmed = line.trim_start();
    let indent_len = line.len() - trimmed.len();

    // 必须以 `- ` 或 `* ` 开头
    if !trimmed.starts_with("- ") && !trimmed.starts_with("* ") {
        return None;
    }

    let rest = &trimmed[2..];

    // 复选框：`[ ]` 或 `[x]` / `[X]`
    if rest.starts_with("[ ] ") {
        Some((false, indent_len + 2 + 4))
    } else if rest.starts_with("[x] ") || rest.starts_with("[X] ") {
        Some((true, indent_len + 2 + 4))
    } else {
        None
    }
}

/// 引号配对，返回所有引号内区间（未闭合的引号不屏蔽）
fn quoted_ranges(line: &str) -> Vec<Span> {
    let mut ranges = Vec::new();
    let mut in_quote = false;
    let mut quote_start = 0;

    for (i, ch) in line.char_indices() {
        if ch == '"' {
            if in_quote {
                ranges.push(Span {
                    start: quote_start,
                    end: i + 1,
                });
                in_quote = false;
            } else {
                quote_start = i;
                in_quote = true;
            }
        }
    }

    // 未闭合的引号不形成区间
    ranges
}

/// Token（分词结果）
#[derive(Debug, Clone)]
struct Token {
    text: String,
    span: Span,
}

/// 分词：按 ASCII 空白分割，引号内区间不分割
fn tokenize(line: &str, start: usize, quoted: &[Span]) -> Vec<Token> {
    let mut tokens = Vec::new();
    let mut current = String::new();
    let mut token_start = start;
    let mut in_token = false;

    for (i, ch) in line[start..].char_indices() {
        let abs_pos = start + i;

        // 检查是否在引号内
        let in_quote = quoted.iter().any(|r| abs_pos >= r.start && abs_pos < r.end);

        if ch == ' ' || ch == '\t' {
            if in_quote {
                // 引号内空白也算 token 一部分
                if !in_token {
                    token_start = abs_pos;
                    in_token = true;
                }
                current.push(ch);
            } else {
                // 引号外空白是分隔符
                if in_token {
                    tokens.push(Token {
                        text: current.clone(),
                        span: Span {
                            start: token_start,
                            end: abs_pos,
                        },
                    });
                    current.clear();
                    in_token = false;
                }
            }
        } else {
            if !in_token {
                token_start = abs_pos;
                in_token = true;
            }
            current.push(ch);
        }
    }

    // 最后一个 token
    if in_token {
        tokens.push(Token {
            text: current,
            span: Span {
                start: token_start,
                end: line.len(),
            },
        });
    }

    tokens
}

/// 右向左扫描元数据区，返回 (标记列表, 降级列表, 正文结束位置)
fn scan_metadata(
    line: &str,
    tokens: &[Token],
    quoted: &[Span],
) -> (Vec<Marker>, Vec<Degraded>, usize) {
    let mut markers = Vec::new();
    let mut degraded = Vec::new();
    let mut seen_kinds = std::collections::HashSet::new();

    let mut i = tokens.len();
    let mut content_end = line.len();
    let mut found_any_marker = false;

    while i > 0 {
        i -= 1;
        let token = &tokens[i];

        // 检查 token 首字节是否在引号内
        let first_byte_quoted = quoted
            .iter()
            .any(|r| token.span.start >= r.start && token.span.start < r.end);

        if first_byte_quoted {
            // 引号内的 token 不是标记，停止扫描
            if found_any_marker {
                content_end = token.span.start;
            }
            break;
        }

        // Case A: token 首字符是标记字符
        if let Some(first_ch) = token.text.chars().next() {
            if matches!(first_ch, '@' | '!' | '#' | '^' | '~') {
                match parse_marker(line, token, quoted) {
                    Ok(marker) => {
                        let kind = marker.value.kind();

                        // 检查重复（# 标签可重复）
                        if kind != MarkerKind::Tag && seen_kinds.contains(&kind) {
                            // 重复的非 tag 标记：停止扫描
                            if found_any_marker {
                                content_end = token.span.start;
                            }
                            break;
                        }

                        seen_kinds.insert(kind);

                        // 每次找到标记都更新 content_end（不断向左推进）
                        content_end = token.span.start;
                        found_any_marker = true;

                        markers.push(marker);
                        continue;
                    }
                    Err(suspected) => {
                        // 非法值：记录降级并停止
                        degraded.push(Degraded {
                            suspected,
                            span: token.span,
                        });
                        if found_any_marker {
                            content_end = token.span.start;
                        }
                        break;
                    }
                }
            }
        }

        // Case B: 尝试与左侧 token 拼接成两词时间表达式
        if i > 0 {
            let prev = &tokens[i - 1];
            if let Some('@') = prev.text.chars().next() {
                // 检查 prev 首字节是否在引号内
                let prev_quoted = quoted
                    .iter()
                    .any(|r| prev.span.start >= r.start && prev.span.start < r.end);

                if !prev_quoted {
                    // 尝试解析 `prev.text + " " + token.text` 为时间
                    let glued = format!("{} {}", prev.text, token.text);
                    if let Ok(time_expr) = parse_time_expr(&glued[1..]) {
                        // 合法：消费两个 token
                        let kind = MarkerKind::Time;
                        if seen_kinds.contains(&kind) {
                            if found_any_marker {
                                content_end = prev.span.start;
                            }
                            break;
                        }

                        seen_kinds.insert(kind);

                        // 每次找到标记都更新 content_end（不断向左推进）
                        content_end = prev.span.start;
                        found_any_marker = true;

                        markers.push(Marker {
                            value: MarkerValue::Time(time_expr),
                            span: Span {
                                start: prev.span.start,
                                end: token.span.end,
                            },
                        });
                        i -= 1; // 额外消费一个
                        continue;
                    }
                }
            }
        }

        // Case C: 既不是标记、也拼不出时间 → 停止
        break;
    }

    // 标记是从右向左收集的，反转成左到右
    markers.reverse();
    degraded.reverse();

    (markers, degraded, content_end)
}

/// 解析单个 token 为标记，返回 Err(kind) 表示疑似该种类但取值非法
fn parse_marker(_line: &str, token: &Token, _quoted: &[Span]) -> Result<Marker, MarkerKind> {
    let text = &token.text;
    let first_ch = text.chars().next().unwrap(); // 调用方已检查非空

    // 提取裸值（去掉首字符）或引号包裹值
    let value_part = &text[1..];
    let unquoted = strip_quotes(value_part);

    let span = token.span;

    match first_ch {
        '@' => parse_time_expr(unquoted)
            .map(|v| Marker {
                value: MarkerValue::Time(v),
                span,
            })
            .map_err(|_| MarkerKind::Time),

        '!' => parse_recurrence(unquoted)
            .map(|v| Marker {
                value: MarkerValue::Repeat(v),
                span,
            })
            .map_err(|_| MarkerKind::Repeat),

        '#' => {
            // 标签值可以是任意非空字符串
            if unquoted.is_empty() {
                Err(MarkerKind::Tag)
            } else {
                Ok(Marker {
                    value: MarkerValue::Tag(unquoted.to_string()),
                    span,
                })
            }
        }

        '^' => parse_intensity(unquoted)
            .map(|v| Marker {
                value: MarkerValue::Intensity(v),
                span,
            })
            .map_err(|_| MarkerKind::Intensity),

        '~' => parse_id(unquoted)
            .map(|v| Marker {
                value: MarkerValue::Id(v.to_string()),
                span,
            })
            .map_err(|_| MarkerKind::Id),

        _ => unreachable!(),
    }
}

/// 去除首尾引号（如果有的话）
fn strip_quotes(s: &str) -> &str {
    if s.starts_with('"') && s.ends_with('"') && s.len() >= 2 {
        &s[1..s.len() - 1]
    } else {
        s
    }
}

/// 解析时间表达式（未求值）
///
/// 支持格式：
/// - `YYYY-MM-DD HH:MM`
/// - `YYYY-MM-DD`
/// - `today HH:MM`
/// - `tomorrow HH:MM`
/// - `today` / `tomorrow`
/// - `HH:MM`
fn parse_time_expr(s: &str) -> Result<TimeExpr, ()> {
    let s = s.trim();

    // 尝试 `today` / `tomorrow`
    if s == "today" {
        return Ok(TimeExpr {
            date: Some(DatePart::Today),
            time: None,
        });
    }
    if s == "tomorrow" {
        return Ok(TimeExpr {
            date: Some(DatePart::Tomorrow),
            time: None,
        });
    }

    // 尝试带空格的组合
    if let Some(space_pos) = s.find(' ') {
        let left = &s[..space_pos];
        let right = &s[space_pos + 1..];

        // `today HH:MM` / `tomorrow HH:MM`
        if left == "today" {
            let time = parse_time_part(right)?;
            return Ok(TimeExpr {
                date: Some(DatePart::Today),
                time: Some(time),
            });
        }
        if left == "tomorrow" {
            let time = parse_time_part(right)?;
            return Ok(TimeExpr {
                date: Some(DatePart::Tomorrow),
                time: Some(time),
            });
        }

        // `YYYY-MM-DD HH:MM`
        if let Ok(date) = parse_absolute_date(left) {
            let time = parse_time_part(right)?;
            return Ok(TimeExpr {
                date: Some(date),
                time: Some(time),
            });
        }
    }

    // 尝试纯日期 `YYYY-MM-DD`
    if let Ok(date) = parse_absolute_date(s) {
        return Ok(TimeExpr {
            date: Some(date),
            time: None,
        });
    }

    // 尝试纯时间 `HH:MM`
    if let Ok(time) = parse_time_part(s) {
        return Ok(TimeExpr {
            date: None,
            time: Some(time),
        });
    }

    Err(())
}

/// 解析绝对日期 `YYYY-MM-DD`
fn parse_absolute_date(s: &str) -> Result<DatePart, ()> {
    let parts: Vec<&str> = s.split('-').collect();
    if parts.len() != 3 {
        return Err(());
    }

    let year: u32 = parts[0].parse().map_err(|_| ())?;
    let month: u32 = parts[1].parse().map_err(|_| ())?;
    let day: u32 = parts[2].parse().map_err(|_| ())?;

    // 简单校验
    if !(2000..=2100).contains(&year) {
        return Err(());
    }
    if !(1..=12).contains(&month) {
        return Err(());
    }
    if day < 1 || day > days_in_month(year, month) {
        return Err(());
    }

    Ok(DatePart::Absolute { year, month, day })
}

/// 解析时间部分 `HH:MM`
fn parse_time_part(s: &str) -> Result<(u32, u32), ()> {
    let parts: Vec<&str> = s.split(':').collect();
    if parts.len() != 2 {
        return Err(());
    }

    let hour: u32 = parts[0].parse().map_err(|_| ())?;
    let minute: u32 = parts[1].parse().map_err(|_| ())?;

    if hour > 23 || minute > 59 {
        return Err(());
    }

    Ok((hour, minute))
}

/// 解析重复规则
///
/// 支持：`once`, `daily`, `weekly`, `monthly`, `yearly`, `weekdays`,
/// `every-<n>-days`, `every-<n>-weeks`
fn parse_recurrence(s: &str) -> Result<Recurrence, ()> {
    match s {
        "once" => Ok(Recurrence::Once),
        "daily" => Ok(Recurrence::Daily),
        "weekly" => Ok(Recurrence::Weekly),
        "monthly" => Ok(Recurrence::Monthly),
        "yearly" => Ok(Recurrence::Yearly),
        "weekdays" => Ok(Recurrence::Weekdays),
        _ => {
            // `every-<n>-days` / `every-<n>-weeks`
            if let Some(rest) = s.strip_prefix("every-") {
                if let Some(n_str) = rest.strip_suffix("-days") {
                    let n: u32 = n_str.parse().map_err(|_| ())?;
                    if n == 0 {
                        return Err(());
                    }
                    return Ok(Recurrence::EveryDays { n });
                }
                if let Some(n_str) = rest.strip_suffix("-weeks") {
                    let n: u32 = n_str.parse().map_err(|_| ())?;
                    if n == 0 {
                        return Err(());
                    }
                    return Ok(Recurrence::EveryWeeks { n });
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

/// 解析 ID（4–8 位十六进制）
fn parse_id(s: &str) -> Result<&str, ()> {
    if s.len() < 4 || s.len() > 8 {
        return Err(());
    }
    if !s.chars().all(|c| c.is_ascii_hexdigit()) {
        return Err(());
    }
    Ok(s)
}

/// 某年某月的天数
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

/// 获取本地时区偏移（秒），简化实现：假定偏移不随时间变化
fn get_local_offset_seconds() -> i64 {
    use std::time::{SystemTime, UNIX_EPOCH};

    // 获取当前 UTC 时间戳
    let now_utc = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64;

    // 使用 libc 的 localtime 获取本地时间信息
    #[cfg(unix)]
    {
        unsafe {
            let mut tm = std::mem::MaybeUninit::<libc::tm>::uninit();
            libc::localtime_r(&now_utc, tm.as_mut_ptr());
            let tm = tm.assume_init();
            tm.tm_gmtoff
        }
    }

    #[cfg(windows)]
    {
        // Windows: 使用 get_timezone 获取时区偏移
        unsafe {
            let mut timezone_seconds: libc::c_long = 0;
            libc::get_timezone(&mut timezone_seconds);

            // get_timezone 返回的是 UTC 比本地时间多的秒数，我们需要相反的符号
            -timezone_seconds as i64
        }
    }
}

/// 从 Unix epoch 以来的天数转换为 (年, 月, 日)
fn days_since_epoch_to_ymd(mut days: i64) -> (u32, u32, u32) {
    // epoch 是 1970-01-01
    let mut year = 1970;

    // 按年推进
    loop {
        let days_in_year = if is_leap(year) { 366 } else { 365 };
        if days < days_in_year {
            break;
        }
        days -= days_in_year;
        year += 1;
    }

    // 按月推进
    let mut month = 1;
    while month <= 12 {
        let days_in_month = days_in_month(year, month);
        if days < days_in_month as i64 {
            break;
        }
        days -= days_in_month as i64;
        month += 1;
    }

    let day = days as u32 + 1; // 日从 1 开始
    (year, month, day)
}

/// 从 (年, 月, 日) 转换为 Unix epoch 以来的天数
fn ymd_to_days_since_epoch(year: u32, month: u32, day: u32) -> Option<i64> {
    // 校验合法性
    if !(1..=12).contains(&month) || day < 1 || day > days_in_month(year, month) {
        return None;
    }

    let mut days = 0i64;

    // 累加 1970 到 year-1 的所有天数
    for y in 1970..year {
        days += if is_leap(y) { 366 } else { 365 };
    }

    // 累加当年 1 月到 month-1 的天数
    for m in 1..month {
        days += days_in_month(year, m) as i64;
    }

    // 加上当月已过的天数
    days += (day - 1) as i64;

    Some(days)
}

/// 时间求值：TimeExpr + 当前时刻 + 默认时间 → 绝对时间戳（秒级 Unix timestamp）
///
/// `now` 是当前时刻（秒级 Unix timestamp），`default_time` 是只指定日期时补全用的 (小时, 分钟)。
///
/// 语义（D4, spec todo/syntax）：
/// - 只有日期：补默认时间
/// - 只有时刻：今天该时刻已过则指明天
/// - 都有：直接组合
///
/// 返回 `None` 表示求值失败（非法日期如 2 月 30 日）。
pub fn evaluate_time(expr: &TimeExpr, now: i64, default_time: (u32, u32)) -> Option<i64> {
    // 将 now 转为本地日期时间（简化：假定本地时区偏移不变）
    let local_offset = get_local_offset_seconds();
    let local_now = now + local_offset;

    // 分解为日期时间分量（基于 Unix epoch 1970-01-01 00:00:00 UTC）
    let days_since_epoch = local_now / 86400;
    let seconds_in_day = (local_now % 86400) as u32;
    let now_hour = seconds_in_day / 3600;
    let now_minute = (seconds_in_day % 3600) / 60;

    let (now_year, now_month, now_day) = days_since_epoch_to_ymd(days_since_epoch);

    // 确定目标日期
    let (target_year, target_month, target_day) = match &expr.date {
        Some(DatePart::Absolute { year, month, day }) => {
            // 校验日期合法性
            if *day > days_in_month(*year, *month) {
                return None;
            }
            (*year, *month, *day)
        }
        Some(DatePart::Today) => (now_year, now_month, now_day),
        Some(DatePart::Tomorrow) => {
            // 加一天
            let (y, m, d) = add_one_day(now_year, now_month, now_day);
            (y, m, d)
        }
        None => {
            // 没有日期：根据时刻判断今天还是明天
            let (target_hour, target_minute) = expr.time.unwrap(); // 不变量保证至少一个 Some
            if target_hour < now_hour || (target_hour == now_hour && target_minute <= now_minute) {
                // 已过，指明天
                add_one_day(now_year, now_month, now_day)
            } else {
                (now_year, now_month, now_day)
            }
        }
    };

    // 确定目标时间
    let (target_hour, target_minute) = expr.time.unwrap_or(default_time);

    // 校验时间合法性
    if target_hour >= 24 || target_minute >= 60 {
        return None;
    }

    // 转回时间戳：年月日 -> days since epoch, 时分 -> seconds in day
    let target_days = ymd_to_days_since_epoch(target_year, target_month, target_day)?;
    let target_seconds_in_day = (target_hour * 3600 + target_minute * 60) as i64;
    let target_local = target_days * 86400 + target_seconds_in_day;

    // 减去本地偏移得到 UTC 时间戳
    Some(target_local - local_offset)
}

/// 日期加一天（自己实现，不引入 chrono 的日期算术）
fn add_one_day(year: u32, month: u32, day: u32) -> (u32, u32, u32) {
    let days_this_month = days_in_month(year, month);
    if day < days_this_month {
        (year, month, day + 1)
    } else {
        // 跨月
        if month < 12 {
            (year, month + 1, 1)
        } else {
            // 跨年
            (year + 1, 1, 1)
        }
    }
}

// ========== 序列化 ==========

impl MarkerValue {
    /// 序列化为规范文本形式。默认值（`!once` / `^toast`）省略不写。
    pub fn serialize(&self) -> String {
        match self {
            Self::Time(expr) => {
                let mut parts = Vec::new();
                if let Some(ref date) = expr.date {
                    match date {
                        DatePart::Absolute { year, month, day } => {
                            parts.push(format!("{:04}-{:02}-{:02}", year, month, day));
                        }
                        DatePart::Today => parts.push("today".to_string()),
                        DatePart::Tomorrow => parts.push("tomorrow".to_string()),
                    }
                }
                if let Some((h, m)) = expr.time {
                    parts.push(format!("{:02}:{:02}", h, m));
                }
                format!("@{}", parts.join(" "))
            }
            Self::Repeat(r) => {
                // 默认值 once 省略不写
                if matches!(r, Recurrence::Once) {
                    return String::new();
                }
                let s = match r {
                    Recurrence::Once => "once",
                    Recurrence::Daily => "daily",
                    Recurrence::Weekly => "weekly",
                    Recurrence::Monthly => "monthly",
                    Recurrence::Yearly => "yearly",
                    Recurrence::Weekdays => "weekdays",
                    Recurrence::EveryDays { n } => return format!("!every-{}d", n),
                    Recurrence::EveryWeeks { n } => return format!("!every-{}w", n),
                };
                format!("!{}", s)
            }
            Self::Tag(tag) => {
                // 如果标签含空格或特殊字符，用引号包裹
                if tag.contains(|c: char| c.is_whitespace() || "@!#^~\"".contains(c)) {
                    format!("#\"{}\"", tag.replace('"', "\\\""))
                } else {
                    format!("#{}", tag)
                }
            }
            Self::Intensity(i) => {
                // 默认值 toast 省略不写
                if matches!(i, Intensity::Toast) {
                    return String::new();
                }
                let s = match i {
                    Intensity::Toast => "toast",
                    Intensity::Ring => "ring",
                    Intensity::Full => "full",
                };
                format!("^{}", s)
            }
            Self::Id(id) => format!("~{}", id),
        }
    }
}

/// 把标记写回到某一行：替换已有同类标记，或追加到元数据区末尾。
///
/// 返回新的行内容。只改动元数据区，正文与其余字节不变。
pub fn write_marker_to_line(line: &str, new_marker: &MarkerValue) -> String {
    let Some(mut todo) = parse(line) else {
        // 不是待办行，原样返回
        return line.to_string();
    };

    let new_kind = new_marker.kind();
    let serialized = new_marker.serialize();

    // 如果是默认值（序列化为空），删除该类标记
    if serialized.is_empty() {
        todo.markers.retain(|m| m.value.kind() != new_kind);
    } else {
        // 查找已有同类标记
        if let Some(existing) = todo.markers.iter_mut().find(|m| m.value.kind() == new_kind) {
            existing.value = new_marker.clone();
        } else {
            // 没有同类标记，追加
            todo.markers.push(Marker {
                value: new_marker.clone(),
                span: Span { start: 0, end: 0 }, // 占位，重建时重算
            });
        }
    }

    // 重建行：前缀 + 正文 + 标记
    let prefix = if todo.checked { "- [x] " } else { "- [ ] " };
    let content = &line[todo.content.start..todo.content.end];
    let markers_text: Vec<String> = todo
        .markers
        .iter()
        .map(|m| m.value.serialize())
        .filter(|s| !s.is_empty())
        .collect();

    if markers_text.is_empty() {
        format!("{}{}", prefix, content)
    } else {
        // 正文和标记之间需要至少一个空格，但如果正文已经以空格结尾则不重复加
        let separator = if content.ends_with(' ') { "" } else { " " };
        format!("{}{}{}{}", prefix, content, separator, markers_text.join(" "))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recognizes_unchecked_todo() {
        let line = "- [ ] 买菜";
        let result = parse(line).unwrap();
        assert!(!result.checked);
        assert_eq!(&line[result.content.start..result.content.end], "买菜");
    }

    #[test]
    fn recognizes_checked_todo() {
        let line = "- [x] 买菜";
        let result = parse(line).unwrap();
        assert!(result.checked);
    }

    #[test]
    fn accepts_asterisk_bullet() {
        let line = "* [ ] 买菜";
        assert!(parse(line).is_some());
    }

    #[test]
    fn accepts_indentation() {
        let line = "  - [ ] 买菜";
        let result = parse(line).unwrap();
        assert_eq!(&line[result.content.start..result.content.end], "买菜");
    }

    #[test]
    fn email_not_mistaken_as_time() {
        let line = "- [ ] 联系 zhang@corp.com";
        let result = parse(line).unwrap();
        assert_eq!(&line[result.content.start..result.content.end], "联系 zhang@corp.com");
        assert!(result.markers.is_empty());
    }

    #[test]
    fn marker_after_content_stops_scan() {
        let line = "- [ ] 交周报 @2026-08-14 记得带U盘";
        let result = parse(line).unwrap();
        assert_eq!(&line[result.content.start..result.content.end], "交周报 @2026-08-14 记得带U盘");
        assert!(result.markers.is_empty());
    }

    #[test]
    fn quoted_literal() {
        let line = r#"- [ ] 告诉他"@明天见" #工作"#;
        let result = parse(line).unwrap();
        assert_eq!(result.markers.len(), 1);
        assert!(matches!(
            result.markers[0].value,
            MarkerValue::Tag(ref s) if s == "工作"
        ));
    }

    #[test]
    fn unclosed_quote_not_masked() {
        let line = r#"- [ ] 草稿："待办管理 @2026-08-15"#;
        let result = parse(line).unwrap();
        // 未闭合引号视为普通字符，@2026-08-15 可以被识别
        assert_eq!(result.markers.len(), 1);
        assert!(matches!(result.markers[0].value, MarkerValue::Time(_)));
    }

    #[test]
    fn invalid_time_degrades() {
        let line = "- [ ] 任务 @明天下午";
        let result = parse(line).unwrap();
        assert_eq!(result.degraded.len(), 1);
        assert_eq!(result.degraded[0].suspected, MarkerKind::Time);
        assert!(result.markers.is_empty());
    }

    #[test]
    fn invalid_recurrence_degrades() {
        let line = "- [ ] 任务 !每天一次";
        let result = parse(line).unwrap();
        assert_eq!(result.degraded.len(), 1);
        assert_eq!(result.degraded[0].suspected, MarkerKind::Repeat);
    }

    #[test]
    fn duplicate_time_stops_scan() {
        let line = "- [ ] 任务 @2026-08-14 @2026-08-15";
        let result = parse(line).unwrap();
        // 第一个时间被识别，第二个时间出现时停止
        assert_eq!(result.markers.len(), 1);
    }

    #[test]
    fn duplicate_tag_allowed() {
        let line = "- [ ] 任务 #项目A #紧急";
        let result = parse(line).unwrap();
        assert_eq!(result.markers.len(), 2);
        assert!(matches!(result.markers[0].value, MarkerValue::Tag(_)));
        assert!(matches!(result.markers[1].value, MarkerValue::Tag(_)));
    }

    #[test]
    fn empty_line_not_todo() {
        assert!(parse("").is_none());
    }

    #[test]
    fn prefix_only_is_todo() {
        let line = "- [ ] ";
        let result = parse(line).unwrap();
        assert_eq!(result.content.start, result.content.end);
    }

    #[test]
    fn all_markers_no_content() {
        let line = "- [ ] @2026-08-14 !daily #标签 ^ring ~a3f9";
        let result = parse(line).unwrap();
        assert_eq!(result.markers.len(), 5);
    }

    #[test]
    fn very_long_line() {
        let long = "- [ ] ".to_string() + &"x".repeat(10000);
        // 不 panic 即可
        let _result = parse(&long);
    }

    #[test]
    fn multibyte_chars() {
        let line = "- [ ] 中文内容 😊 @2026-08-14";
        let result = parse(line).unwrap();
        assert!(result.markers.len() > 0);
    }

    #[test]
    fn two_word_time() {
        let line = "- [ ] 任务 @2026-08-14 18:00";
        let result = parse(line).unwrap();
        assert_eq!(result.markers.len(), 1);
        if let MarkerValue::Time(ref expr) = result.markers[0].value {
            assert!(expr.date.is_some());
            assert_eq!(expr.time, Some((18, 0)));
        } else {
            panic!("expected Time marker");
        }
    }

    // === 时间求值测试 ===

    #[test]
    fn evaluate_absolute_datetime() {
        let expr = TimeExpr {
            date: Some(DatePart::Absolute {
                year: 2026,
                month: 12,
                day: 25,
            }),
            time: Some((14, 30)),
        };
        // 不管 now 是什么，绝对时间就是那个时刻
        let ts = evaluate_time(&expr, 0, (9, 0)).unwrap();
        // 验证结果：反向转换回日期时间
        let offset = get_local_offset_seconds();
        let local_ts = ts + offset;
        let days = local_ts / 86400;
        let secs = (local_ts % 86400) as u32;
        let (y, m, d) = days_since_epoch_to_ymd(days);
        let hour = secs / 3600;
        let minute = (secs % 3600) / 60;
        assert_eq!((y, m, d), (2026, 12, 25));
        assert_eq!((hour, minute), (14, 30));
    }

    #[test]
    fn evaluate_only_date_fills_default_time() {
        let expr = TimeExpr {
            date: Some(DatePart::Absolute {
                year: 2026,
                month: 8,
                day: 14,
            }),
            time: None,
        };
        let ts = evaluate_time(&expr, 0, (11, 45)).unwrap();
        let offset = get_local_offset_seconds();
        let local_ts = ts + offset;
        let secs = (local_ts % 86400) as u32;
        let hour = secs / 3600;
        let minute = (secs % 3600) / 60;
        assert_eq!((hour, minute), (11, 45));
    }

    #[test]
    fn evaluate_only_time_before_now_points_to_today() {
        // 构造「现在是 2026-08-14 17:59」本地时间的 UTC 时间戳
        let offset = get_local_offset_seconds();
        let days = ymd_to_days_since_epoch(2026, 8, 14).unwrap();
        let secs_in_day = 17 * 3600 + 59 * 60;
        let local_ts = days * 86400 + secs_in_day;
        let now_ts = local_ts - offset;

        // 18:00 还没到，应该指今天
        let expr = TimeExpr {
            date: None,
            time: Some((18, 0)),
        };
        let ts = evaluate_time(&expr, now_ts, (9, 0)).unwrap();
        let result_local = ts + offset;
        let result_days = result_local / 86400;
        let (y, m, d) = days_since_epoch_to_ymd(result_days);
        assert_eq!((y, m, d), (2026, 8, 14)); // 今天
    }

    #[test]
    fn evaluate_only_time_after_now_points_to_tomorrow() {
        // 构造「现在是 2026-08-14 18:01」
        let offset = get_local_offset_seconds();
        let days = ymd_to_days_since_epoch(2026, 8, 14).unwrap();
        let secs_in_day = 18 * 3600 + 1 * 60;
        let local_ts = days * 86400 + secs_in_day;
        let now_ts = local_ts - offset;

        // 18:00 已过，应该指明天
        let expr = TimeExpr {
            date: None,
            time: Some((18, 0)),
        };
        let ts = evaluate_time(&expr, now_ts, (9, 0)).unwrap();
        let result_local = ts + offset;
        let result_days = result_local / 86400;
        let (y, m, d) = days_since_epoch_to_ymd(result_days);
        assert_eq!((y, m, d), (2026, 8, 15)); // 明天
    }

    #[test]
    fn evaluate_today_and_tomorrow() {
        // 构造「现在是 2026-08-14 12:00」
        let offset = get_local_offset_seconds();
        let days = ymd_to_days_since_epoch(2026, 8, 14).unwrap();
        let secs_in_day = 12 * 3600;
        let local_ts = days * 86400 + secs_in_day;
        let now_ts = local_ts - offset;

        let today_expr = TimeExpr {
            date: Some(DatePart::Today),
            time: Some((15, 0)),
        };
        let ts = evaluate_time(&today_expr, now_ts, (9, 0)).unwrap();
        let result_local = ts + offset;
        let result_days = result_local / 86400;
        let (y, m, d) = days_since_epoch_to_ymd(result_days);
        assert_eq!((y, m, d), (2026, 8, 14)); // 今天

        let tomorrow_expr = TimeExpr {
            date: Some(DatePart::Tomorrow),
            time: Some((10, 0)),
        };
        let ts = evaluate_time(&tomorrow_expr, now_ts, (9, 0)).unwrap();
        let result_local = ts + offset;
        let result_days = result_local / 86400;
        let (y, m, d) = days_since_epoch_to_ymd(result_days);
        assert_eq!((y, m, d), (2026, 8, 15)); // 明天
    }

    #[test]
    fn evaluate_add_one_day_across_month_and_year() {
        // 跨月
        assert_eq!(add_one_day(2026, 8, 31), (2026, 9, 1));
        // 跨年
        assert_eq!(add_one_day(2026, 12, 31), (2027, 1, 1));
        // 平年 2 月
        assert_eq!(add_one_day(2027, 2, 28), (2027, 3, 1));
        // 闰年 2 月
        assert_eq!(add_one_day(2024, 2, 29), (2024, 3, 1));
    }

    #[test]
    fn evaluate_rejects_invalid_date() {
        let expr = TimeExpr {
            date: Some(DatePart::Absolute { year: 2026, month: 2, day: 30 }),
            time: Some((10, 0)),
        };
        assert!(evaluate_time(&expr, 0, (9, 0)).is_none());
    }

    // === 序列化测试 ===

    #[test]
    fn serialize_time_variants() {
        let expr1 = TimeExpr {
            date: Some(DatePart::Absolute { year: 2026, month: 8, day: 14 }),
            time: Some((18, 0)),
        };
        assert_eq!(MarkerValue::Time(expr1).serialize(), "@2026-08-14 18:00");

        let expr2 = TimeExpr {
            date: Some(DatePart::Today),
            time: None,
        };
        assert_eq!(MarkerValue::Time(expr2).serialize(), "@today");

        let expr3 = TimeExpr {
            date: None,
            time: Some((9, 30)),
        };
        assert_eq!(MarkerValue::Time(expr3).serialize(), "@09:30");
    }

    #[test]
    fn serialize_recurrence_omits_default() {
        assert_eq!(MarkerValue::Repeat(Recurrence::Once).serialize(), "");
        assert_eq!(MarkerValue::Repeat(Recurrence::Daily).serialize(), "!daily");
        assert_eq!(MarkerValue::Repeat(Recurrence::EveryDays { n: 3 }).serialize(), "!every-3d");
    }

    #[test]
    fn serialize_tag_quotes_when_needed() {
        assert_eq!(MarkerValue::Tag("工作".to_string()).serialize(), "#工作");
        assert_eq!(
            MarkerValue::Tag("项目 A".to_string()).serialize(),
            "#\"项目 A\""
        );
        assert_eq!(
            MarkerValue::Tag("含@符号".to_string()).serialize(),
            "#\"含@符号\""
        );
    }

    #[test]
    fn serialize_intensity_omits_default() {
        assert_eq!(MarkerValue::Intensity(Intensity::Toast).serialize(), "");
        assert_eq!(MarkerValue::Intensity(Intensity::Ring).serialize(), "^ring");
        assert_eq!(MarkerValue::Intensity(Intensity::Full).serialize(), "^full");
    }

    #[test]
    fn serialize_id() {
        assert_eq!(MarkerValue::Id("a3f9".to_string()).serialize(), "~a3f9");
    }

    #[test]
    fn write_marker_replaces_existing() {
        let line = "- [ ] 买菜 @2026-08-14";
        let new_time = MarkerValue::Time(TimeExpr {
            date: Some(DatePart::Absolute { year: 2026, month: 8, day: 15 }),
            time: Some((10, 0)),
        });
        let result = write_marker_to_line(line, &new_time);
        assert_eq!(result, "- [ ] 买菜 @2026-08-15 10:00");
    }

    #[test]
    fn write_marker_appends_new_kind() {
        let line = "- [ ] 买菜 @2026-08-14";
        let tag = MarkerValue::Tag("生活".to_string());
        let result = write_marker_to_line(line, &tag);
        assert_eq!(result, "- [ ] 买菜 @2026-08-14 #生活");
    }

    #[test]
    fn write_marker_removes_when_default() {
        let line = "- [ ] 买菜 @2026-08-14 ^ring";
        let default_intensity = MarkerValue::Intensity(Intensity::Toast);
        let result = write_marker_to_line(line, &default_intensity);
        assert_eq!(result, "- [ ] 买菜 @2026-08-14");
    }

    #[test]
    fn write_marker_preserves_content_exactly() {
        let line = "- [ ] 发邮件给张三  @2026-08-14"; // 两个空格
        let tag = MarkerValue::Tag("工作".to_string());
        let result = write_marker_to_line(line, &tag);
        // 正文里的两个空格必须保留
        assert!(result.starts_with("- [ ] 发邮件给张三  "));
    }

    #[test]
    fn roundtrip_parse_and_serialize() {
        let line = "- [ ] 任务 @2026-08-14 18:00 !daily #工作 ^ring ~a3f9";
        let todo = parse(line).unwrap();

        // 重建
        let rebuilt = write_marker_to_line(line, &MarkerValue::Time(TimeExpr {
            date: Some(DatePart::Absolute { year: 2026, month: 8, day: 14 }),
            time: Some((18, 0)),
        }));

        // 应该能再次解析
        let reparsed = parse(&rebuilt).unwrap();
        assert_eq!(reparsed.markers.len(), todo.markers.len());
    }
}
