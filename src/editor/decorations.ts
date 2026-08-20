// 待办标记的 chip 渲染层（D16 / design E4-E6）
//
// 底线：渲染层只在文本之上叠加，永不改写文本。这里所有 Decoration 都是
// 纯显示叠加，不产生任何事务。
//
// 坐标系有两套，别搞混：
//   - Rust 解析器返回的 Span 是 **UTF-8 字节偏移，闭区间 [start, end]**
//   - CM6 用的是 **UTF-16 code unit 偏移，开区间 [from, to)**
// 转换只在 `byteToUtf16` 这一处做，其余地方一律已是 CM6 坐标。

import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import { Facet, StateEffect, type Range } from '@codemirror/state';
import { syntaxTree } from '@codemirror/language';
import {
  Decoration,
  EditorView,
  ViewPlugin,
  WidgetType,
  type DecorationSet,
  type ViewUpdate,
} from '@codemirror/view';
import type { Intensity, MarkerValue, Recurrence, TimeExpr, TodoLine } from '../types/todo';

// ---------- 解析结果缓存（design E4） ----------
//
// 解析是纯函数，所以行文本本身就是合法的缓存键。undefined = 还没解析过，
// null = 解析过但不是待办行。

const cache = new Map<string, TodoLine | null>();
const CACHE_LIMIT = 5000;

function remember(text: string, parsed: TodoLine | null) {
  if (cache.size >= CACHE_LIMIT) cache.clear();
  cache.set(text, parsed);
}

/** 打开文件时批量填缓存，避免滚动时逐屏补解析。 */
export async function primeParseCache(lines: string[]): Promise<void> {
  const missing = [...new Set(lines.filter((l) => !cache.has(l)))];
  if (missing.length === 0) return;
  try {
    const results = await invoke<(TodoLine | null)[]>('parse_todo_lines', { lines: missing });
    missing.forEach((text, i) => remember(text, results[i] ?? null));
  } catch {
    // 解析不可用不是致命错误：编辑器退化为纯文本，编辑与保存照常（5.8）。
  }
}

const parsedEffect = StateEffect.define<null>();

/** 未命中行的异步解析队列。一批一次 invoke，绝不逐行请求。 */
class ParseQueue {
  private pending = new Set<string>();
  private timer: number | null = null;
  private consecutiveFailures = 0;
  private disposed = false;

  constructor(private readonly onResolved: () => void) {}

  /** 解析链路连续失败后停止请求，编辑器退化为纯文本编辑器（5.8）。 */
  private get available() {
    return this.consecutiveFailures < 3;
  }

  request(text: string) {
    if (this.disposed || !this.available || this.pending.has(text)) return;
    this.pending.add(text);
    if (this.timer === null) {
      // 攒一拍再发：一次 update 里几十行未命中只会产生一个请求。
      this.timer = window.setTimeout(() => void this.run(), 0);
    }
  }

  private async run() {
    this.timer = null;
    const batch = [...this.pending];
    this.pending.clear();
    if (batch.length === 0) return;

    try {
      const results = await invoke<(TodoLine | null)[]>('parse_todo_lines', { lines: batch });
      batch.forEach((text, i) => remember(text, results[i] ?? null));
      this.consecutiveFailures = 0;
      // 请求飞在半空时编辑器可能已被销毁（切换笔记/切换工作区），
      // 这时 dispatch 会抛错。结果仍写进缓存，下次打开直接命中。
      if (!this.disposed) this.onResolved();
    } catch {
      this.consecutiveFailures += 1;
    }
  }

  dispose() {
    this.disposed = true;
    if (this.timer !== null) clearTimeout(this.timer);
    this.timer = null;
    this.pending.clear();
  }
}

// ---------- 坐标转换 ----------

function utf8Len(codePoint: number): number {
  if (codePoint < 0x80) return 1;
  if (codePoint < 0x800) return 2;
  if (codePoint < 0x10000) return 3;
  return 4;
}

/**
 * 造一个「UTF-8 字节偏移 → UTF-16 偏移」的查表函数。
 * 偏移不在字符边界上时返回 undefined——宁可这处不渲染，也不错位。
 */
function byteToUtf16(line: string): (byte: number) => number | undefined {
  let ascii = true;
  for (let i = 0; i < line.length; i += 1) {
    if (line.charCodeAt(i) > 0x7f) {
      ascii = false;
      break;
    }
  }
  // 纯 ASCII 行两套坐标完全一致，省掉建表。
  if (ascii) return (byte) => (byte >= 0 && byte <= line.length ? byte : undefined);

  const map = new Map<number, number>();
  let byte = 0;
  let u16 = 0;
  for (const ch of line) {
    map.set(byte, u16);
    byte += utf8Len(ch.codePointAt(0)!);
    u16 += ch.length;
  }
  map.set(byte, u16);
  return (b) => map.get(b);
}

// ---------- 显示文案格式化（design E5） ----------
//
// 这层只影响 chip 上显示什么字。**它的产物绝不进入文档**——任何要写回
// 文件的文本都由 Rust 的 serialize_marker 产出。

const pad2 = (n: number) => String(n).padStart(2, '0');

function formatTime(t: TimeExpr): string {
  const parts: string[] = [];
  if (t.date) {
    if (t.date.kind === 'absolute') parts.push(`${t.date.month}月${t.date.day}日`);
    else if (t.date.kind === 'today') parts.push('今天');
    else parts.push('明天');
  }
  if (t.time) parts.push(`${pad2(t.time[0])}:${pad2(t.time[1])}`);
  return parts.join(' ');
}

function formatRepeat(r: Recurrence): string {
  switch (r.kind) {
    case 'once':
      return '一次';
    case 'daily':
      return '每天';
    case 'weekly':
      return '每周';
    case 'monthly':
      return '每月';
    case 'yearly':
      return '每年';
    case 'weekdays':
      return '工作日';
    case 'every_days':
      return `每 ${r.n} 天`;
    case 'every_weeks':
      return `每 ${r.n} 周`;
  }
}

const INTENSITY_LABEL: Record<Intensity, string> = {
  toast: '轻提醒',
  ring: '响铃',
  full: '全屏',
};

// ---------- Widget ----------

class ChipWidget extends WidgetType {
  constructor(
    private readonly variant: string,
    private readonly icon: string,
    private readonly label: string,
    private readonly raw: string,
  ) {
    super();
  }

  eq(other: ChipWidget) {
    return other.variant === this.variant && other.label === this.label;
  }

  toDOM() {
    const chip = document.createElement('span');
    chip.className = `ni-chip ni-chip-${this.variant}`;
    // 悬浮显示原始语法：渲染态看不见原文时，这是最快的自查手段。
    chip.title = `${this.raw}（点击修改）`;
    chip.dataset.raw = this.raw;
    // ~id 不可点：它是身份锚点，不该由用户随手改
    if (this.variant !== 'id') chip.dataset.kind = this.variant;

    if (this.icon !== '') {
      const icon = document.createElement('span');
      icon.className = 'ni-chip-icon';
      icon.textContent = this.icon;
      chip.append(icon);
    }
    const text = document.createElement('span');
    text.textContent = this.label;
    chip.append(text);
    return chip;
  }

  ignoreEvent() {
    return false;
  }
}

// ---------- decoration 构建 ----------

// `~id` 完全隐藏：replace 且不给 widget，不占任何宽度。
const hidden = Decoration.replace({});
const degradedMark = Decoration.mark({
  class: 'ni-degraded',
  attributes: { title: '这个标记的值无法识别，已按普通文本处理。想原样显示可以用引号包起来。' },
});

function chipFor(marker: TodoLine['markers'][number], raw: string): Decoration | null {
  const v = marker.value;
  switch (v.kind) {
    case 'time':
      return Decoration.replace({ widget: new ChipWidget('time', '📅', formatTime(v.value), raw) });
    case 'repeat':
      return Decoration.replace({
        widget: new ChipWidget('repeat', '🔁', formatRepeat(v.value), raw),
      });
    case 'tag':
      return Decoration.replace({ widget: new ChipWidget('tag', '', v.value, raw) });
    case 'intensity':
      return Decoration.replace({
        widget: new ChipWidget('intensity', '🔔', INTENSITY_LABEL[v.value] ?? v.value, raw),
      });
    case 'id':
      return hidden;
    default:
      return null;
  }
}

/**
 * 一行的装饰画布。
 *
 * 所有坐标都是**文档绝对偏移**。`taken` 记录已被 replace 占用的区间：
 * CM6 里两个 replace 重叠会在绘制时抛错、整个编辑器白屏，所以 chip 与
 * markdown 语法隐藏必须共用同一份占用表——宁可少渲染一处，也不能崩。
 */
class LineCanvas {
  private readonly taken: Array<[number, number]> = [];

  constructor(private readonly out: Range<Decoration>[]) {}

  private overlaps(from: number, to: number) {
    return this.taken.some(([a, b]) => from < b && to > a);
  }

  /** 占位并替换（chip / 隐藏）。被占用则放弃这一处。 */
  replace(from: number, to: number, deco: Decoration): boolean {
    if (to <= from || this.overlaps(from, to)) return false;
    this.taken.push([from, to]);
    this.out.push(deco.range(from, to));
    return true;
  }

  /** 叠加样式，不占位；落在已隐藏区间上没有意义，故同样跳过。 */
  mark(from: number, to: number, deco: Decoration) {
    if (to <= from || this.overlaps(from, to)) return;
    this.out.push(deco.range(from, to));
  }

  line(pos: number, deco: Decoration) {
    this.out.push(deco.range(pos));
  }
}

function decorateTodoLine(
  canvas: LineCanvas,
  lineStart: number,
  lineText: string,
  parsed: TodoLine,
) {
  const b2u = byteToUtf16(lineText);

  for (const marker of parsed.markers) {
    const from = b2u(marker.span.start);
    const to = b2u(marker.span.end + 1);
    // 边界对不上说明解析结果与这行文本不是一回事，跳过而不是错位渲染。
    if (from === undefined || to === undefined) continue;
    const deco = chipFor(marker, lineText.slice(from, to));
    if (deco !== null) canvas.replace(lineStart + from, lineStart + to, deco);
  }

  // 引号本身在渲染态隐藏，引号内的文本照常显示（5.5）
  for (const q of parsed.quoted) {
    const open = b2u(q.start);
    const close = b2u(q.end);
    if (open === undefined || close === undefined || close <= open) continue;
    canvas.replace(lineStart + open, lineStart + open + 1, hidden);
    canvas.replace(lineStart + close, lineStart + close + 1, hidden);
  }

  // 降级标记：轻量警告下划线，不成 chip、不阻塞编辑（5.6）
  for (const bad of parsed.degraded) {
    const from = b2u(bad.span.start);
    const to = b2u(bad.span.end + 1);
    if (from === undefined || to === undefined) continue;
    canvas.mark(lineStart + from, lineStart + to, degradedMark);
  }
}

// ---------- 交互：复选框与 chip（design E7） ----------
//
// 所有修改都走编辑缓冲，不另开写路径：同一行若同时存在「缓冲里的版本」和
// 「被 actor 直接改过的磁盘版本」，就多一类竞态，而省下的只是一次缓冲更新。

/** 宿主注入的「立即落盘」回调。点复选框、改 chip 是明确动作，不等 800ms。 */
export const flushRequest = Facet.define<() => void, () => void>({
  combine: (values) => values[0] ?? (() => {}),
});

/**
 * 求两段文本的最小差异区间。
 *
 * Rust 那边返回的是**整行新文本**（语法规则只有一份，前端不该自己拼），
 * 但直接整行替换会让 ChangeSet 里塞满没改过的字节。这里收敛成一处连续
 * 改动，既满足「该行其余字节不变」，也让 apply_edits 的 payload 保持精简。
 */
export function minimalChange(
  oldText: string,
  newText: string,
): { from: number; to: number; insert: string } | null {
  if (oldText === newText) return null;

  const max = Math.min(oldText.length, newText.length);
  let start = 0;
  while (start < max && oldText[start] === newText[start]) start += 1;

  let endOld = oldText.length;
  let endNew = newText.length;
  while (endOld > start && endNew > start && oldText[endOld - 1] === newText[endNew - 1]) {
    endOld -= 1;
    endNew -= 1;
  }

  // 别把代理对劈成两半：边界落在低位代理上就各退一格。
  const isLow = (s: string, i: number) => {
    const c = s.charCodeAt(i);
    return c >= 0xdc00 && c <= 0xdfff;
  };
  if (start > 0 && start < oldText.length && isLow(oldText, start)) start -= 1;
  if (endOld < oldText.length && isLow(oldText, endOld)) endOld += 1;
  if (endNew < newText.length && isLow(newText, endNew)) endNew += 1;

  return { from: start, to: endOld, insert: newText.slice(start, endNew) };
}

/** 把 Rust 算出的新行文本落进缓冲，并立即请求落盘。 */
function applyNewLineText(view: EditorView, lineFrom: number, oldText: string, newText: string) {
  const change = minimalChange(oldText, newText);
  if (change === null) return;
  view.dispatch({
    changes: {
      from: lineFrom + change.from,
      to: lineFrom + change.to,
      insert: change.insert,
    },
  });
  view.state.facet(flushRequest)();
}

class CheckboxWidget extends WidgetType {
  constructor(private readonly checked: boolean) {
    super();
  }

  eq(other: CheckboxWidget) {
    return other.checked === this.checked;
  }

  toDOM() {
    const box = document.createElement('span');
    box.className = `ni-checkbox${this.checked ? ' ni-checkbox-on' : ''}`;
    box.textContent = this.checked ? '✓' : '';
    box.title = this.checked ? '点击标记为未完成' : '点击标记为完成';
    return box;
  }

  ignoreEvent() {
    return false; // 让点击事件传到 domEventHandlers
  }
}

async function toggleCheckboxAt(view: EditorView, pos: number) {
  const line = view.state.doc.lineAt(pos);
  try {
    const next = await invoke<string>('toggle_checkbox', { line: line.text });
    applyNewLineText(view, line.from, line.text, next);
  } catch {
    // 不是待办行等情况：什么都不做，绝不猜着改文本
  }
}

// ---------- chip 点击 → 独立选择器窗口 ----------
//
// 选中结果是广播，速记条也在听，所以每次打开都带一个请求 id，回来时对不上
// 就不是自己那一次。窗口只有一个，同一时刻只可能有一个待处理请求。

let pendingEdit: { view: EditorView; lineFrom: number; requestId: string } | null = null;
let pickerSeq = 0;

async function openChipPicker(view: EditorView, pos: number, kind: string) {
  const line = view.state.doc.lineAt(pos);
  pickerSeq += 1;
  const requestId = `editor-${pickerSeq}`;
  pendingEdit = { view, lineFrom: line.from, requestId };
  try {
    await invoke('open_marker_picker', { kind, requestId });
  } catch {
    pendingEdit = null;
  }
}

void listen<{ requestId: string; value: MarkerValue }>('marker-picker:selected', async (event) => {
  const target = pendingEdit;
  if (target === null || event.payload.requestId !== target.requestId) return;
  pendingEdit = null;

  const { view, lineFrom } = target;
  // 行号可能已经漂了（比如期间来了远端变更），重新按当前文档取这一行。
  if (lineFrom > view.state.doc.length) return;
  const line = view.state.doc.lineAt(lineFrom);

  try {
    let next = await invoke<string>('write_marker', {
      line: line.text,
      value: event.payload.value,
    });

    // 6.6：设了提醒却还没有 ~id 的待办，顺手分配一个。ID 是 md 行与 DB 行的
    // 映射锚点，没有它提醒引擎认不出这条待办。和标记写在同一次缓冲变更里，
    // 落盘时是一次 apply_edits，不会出现「有提醒但没 ID」的中间态。
    if (event.payload.value.kind === 'time') {
      const parsed = await invoke<TodoLine | null>('parse_todo_line', { text: next });
      const hasId = parsed?.markers.some((m) => m.value.kind === 'id') ?? false;
      if (parsed !== null && !hasId) {
        const id = await invoke<string>('allocate_todo_id');
        next = await invoke<string>('write_marker', {
          line: next,
          value: { kind: 'id', value: id },
        });
      }
    }

    applyNewLineText(view, line.from, line.text, next);
  } catch (err) {
    console.error('写入标记失败:', err);
  }
});

/** 该行是否与任一选区相交（相交则显示原始语法，design E6）。 */
function touchedBySelection(view: EditorView, from: number, to: number): boolean {
  return view.state.selection.ranges.some((r) => r.from <= to && r.to >= from);
}

// ---------- 通用 markdown 渲染（Typora 式，D11） ----------
//
// 复用 lang-markdown 已经建好的语法树，不引入第二个解析器。这里只做两件事：
//   1. 把语法符号（`#` `**` `` ` `` `>` `~~` `[]()`）隐藏
//   2. 给块级元素挂 class，字号/边框交给 CSS
// 一个字节都不改文档——光标一进这行，所有符号原样回来。

const headingLine = [1, 2, 3, 4, 5, 6].map((n) =>
  Decoration.line({ class: `ni-h${n}` }),
);
const quoteLine = Decoration.line({ class: 'ni-quote' });
const strikeMark = Decoration.mark({ class: 'ni-strike' });
const linkTextMark = Decoration.mark({ class: 'ni-link' });
const inlineCodeMark = Decoration.mark({ class: 'ni-inline-code' });
const codeBlockLine = Decoration.line({ class: 'ni-code-block' });
const codeBlockFirst = Decoration.line({ class: 'ni-code-first' });
const codeBlockLast = Decoration.line({ class: 'ni-code-last' });

/** 围栏代码块的语言标签（`​```http` 里的 http）。 */
class LangBadgeWidget extends WidgetType {
  constructor(private readonly lang: string) {
    super();
  }

  eq(other: LangBadgeWidget) {
    return other.lang === this.lang;
  }

  toDOM() {
    const badge = document.createElement('span');
    badge.className = 'ni-code-lang';
    badge.textContent = this.lang;
    return badge;
  }
}

/** 隐藏范围，并顺带吃掉紧跟其后的一个空格（`# ` / `> ` 留着空格会顶出缩进）。 */
function hideWithTrailingSpace(canvas: LineCanvas, view: EditorView, from: number, to: number) {
  const next = view.state.doc.sliceString(to, to + 1);
  canvas.replace(from, next === ' ' ? to + 1 : to, hidden);
}

/**
 * @param rendered 该行是否处于渲染态（未被选区触及）。
 *
 * 代码块的背景与边框**不受它影响**：那是块级样式而非语法符号，光标一进去
 * 就让整个代码块散架，比看见 ``` 更难受。
 */
function decorateMarkdown(
  canvas: LineCanvas,
  view: EditorView,
  lineFrom: number,
  lineTo: number,
  rendered: boolean,
) {
  syntaxTree(view.state).iterate({
    from: lineFrom,
    to: lineTo,
    enter(node) {
      if (node.name === 'FencedCode') {
        canvas.line(lineFrom, codeBlockLine);
        // 首尾行拿圆角与内边距，代码块的起止因此始终可见——这也是能放心
        // 藏掉 ``` 的前提。
        if (node.from >= lineFrom && node.from <= lineTo) {
          canvas.line(lineFrom, codeBlockFirst);
        }
        if (node.to >= lineFrom && node.to <= lineTo) {
          canvas.line(lineFrom, codeBlockLast);
        }
        return;
      }

      if (!rendered) return;

      switch (node.name) {
        case 'ATXHeading1':
        case 'ATXHeading2':
        case 'ATXHeading3':
        case 'ATXHeading4':
        case 'ATXHeading5':
        case 'ATXHeading6': {
          const level = Number(node.name.slice(-1));
          canvas.line(lineFrom, headingLine[level - 1]);
          break;
        }
        case 'Blockquote':
          canvas.line(lineFrom, quoteLine);
          break;
        case 'HeaderMark':
        case 'QuoteMark':
          hideWithTrailingSpace(canvas, view, node.from, node.to);
          break;
        case 'EmphasisMark':
        case 'StrikethroughMark':
        case 'LinkMark':
          canvas.replace(node.from, node.to, hidden);
          break;
        case 'InlineCode':
          // 先上底色再藏反引号：mark 会避开已被 replace 占用的区间，顺序反了就落不下
          canvas.mark(node.from, node.to, inlineCodeMark);
          break;
        case 'CodeMark':
          // 行内代码的反引号、围栏代码块的 ``` 都藏掉。
          // 藏 ``` 是安全的：块背景与首尾圆角已经把边界画出来了。
          canvas.replace(node.from, node.to, hidden);
          break;
        case 'CodeInfo':
          // `​```http` 的语言名换成小徽标，比一行光秃秃的 ``` 有用
          canvas.replace(
            node.from,
            node.to,
            Decoration.replace({
              widget: new LangBadgeWidget(view.state.doc.sliceString(node.from, node.to)),
            }),
          );
          break;
        case 'URL': {
          // 只有 `[文字](地址)` 这种带文字的才收起地址；裸 URL / 自动链接
          // 没有可显示的替代文字，藏了等于内容凭空消失。
          const parent = node.node.parent?.name;
          if (parent === 'Link' || parent === 'Image') {
            canvas.replace(node.from, node.to, hidden);
          }
          break;
        }
        case 'Strikethrough':
          canvas.mark(node.from, node.to, strikeMark);
          break;
        case 'TaskMarker': {
          // GFM 的 `[ ]` / `[x]`。换成可点的方块，点击走 Rust 只改那一个字符。
          const checked = view.state.doc.sliceString(node.from, node.to).toLowerCase() === '[x]';
          canvas.replace(
            node.from,
            node.to,
            Decoration.replace({ widget: new CheckboxWidget(checked) }),
          );
          break;
        }
        case 'Link':
          canvas.mark(node.from, node.to, linkTextMark);
          break;
        default:
          break;
      }
    },
  });
}

function buildDecorations(view: EditorView, queue: ParseQueue): DecorationSet {
  const out: Range<Decoration>[] = [];

  for (const visible of view.visibleRanges) {
    let pos = visible.from;
    while (pos <= visible.to) {
      const line = view.state.doc.lineAt(pos);
      const canvas = new LineCanvas(out);
      const rendered = !touchedBySelection(view, line.from, line.to);

      // 光标/选区所在行显示原文，连解析都不必等
      if (rendered) {
        // chip 先占位：待办标记比 markdown 语法更要紧，冲突时它赢。
        const parsed = cache.get(line.text);
        if (parsed === undefined) {
          // 还没解析：这一轮按原文显示，结果回来后再重算（5.2）
          if (line.text !== '') queue.request(line.text);
        } else if (parsed !== null) {
          decorateTodoLine(canvas, line.from, line.text, parsed);
        }
      }

      // 即使在选区行也要走一趟：代码块的块级样式与选区无关
      decorateMarkdown(canvas, view, line.from, line.to, rendered);

      if (line.to + 1 <= pos) break; // 防御：空文档时不空转
      pos = line.to + 1;
    }
  }

  return Decoration.set(out, true);
}

export const todoChips = ViewPlugin.fromClass(
  class {
    decorations: DecorationSet;
    private readonly queue: ParseQueue;

    constructor(view: EditorView) {
      this.queue = new ParseQueue(() => {
        // 空事务只带 effect：不改文档，因此不会被当成一次编辑去提交。
        view.dispatch({ effects: parsedEffect.of(null) });
      });
      this.decorations = buildDecorations(view, this.queue);
    }

    update(update: ViewUpdate) {
      const parsedArrived = update.transactions.some((tr) =>
        tr.effects.some((e) => e.is(parsedEffect)),
      );
      // 语法树变了也要重算：语言包是按需异步加载的（```rust 要等 lang-rust
      // 到位），大文件的 markdown 树本身也是渐进解析出来的。不看这一条，
      // 首屏那一版不完整的树会一直留在屏幕上。
      const treeChanged = syntaxTree(update.startState) !== syntaxTree(update.state);

      if (
        update.docChanged ||
        update.selectionSet ||
        update.viewportChanged ||
        parsedArrived ||
        treeChanged
      ) {
        this.decorations = buildDecorations(update.view, this.queue);
      }
    }

    destroy() {
      this.queue.dispose();
    }
  },
  { decorations: (v) => v.decorations },
);

/**
 * 复选框点击。
 *
 * 用 mousedown 而不是 click：click 之前光标已经落进这一行，该行会立刻
 * 切回原始语法、复选框 widget 随之消失，点击就落空了。
 */
export const markerInteraction = EditorView.domEventHandlers({
  mousedown(event, view) {
    const target = event.target as HTMLElement | null;

    const box = target?.closest('.ni-checkbox');
    if (box !== null && box !== undefined) {
      void toggleCheckboxAt(view, view.posAtDOM(box));
      return true; // 阻止默认行为，光标不进入该行
    }

    const chip = target?.closest<HTMLElement>('.ni-chip');
    if (chip !== null && chip !== undefined) {
      const kind = chip.dataset.kind;
      if (kind !== undefined) {
        void openChipPicker(view, view.posAtDOM(chip), kind);
        return true;
      }
    }

    return false;
  },
});
