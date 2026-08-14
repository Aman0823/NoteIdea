import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import * as assist from './assist';
import type { TodoLine } from './types/todo';

const input = document.querySelector<HTMLInputElement>('#input')!;
const latency = document.querySelector<HTMLSpanElement>('#latency')!;

// 解析请求序号（D2：只采用最新响应）
let parseSeq = 0;

/**
 * 终点判定：必须同时满足
 *   1. document.hasFocus()  —— 键盘焦点真的到了这个 WebView
 *   2. activeElement === input —— 焦点在输入框上
 *
 * 只等一帧绘制是不够的：窗口可以画出来但键盘焦点还没交过来，
 * 此时敲键盘没反应。之前的测量就漏了这一段。
 */
const FOCUS_TIMEOUT_MS = 3000;

function awaitTypable() {
  const t0 = performance.now();
  let frames = 0;

  const tick = () => {
    frames++;
    const focused = document.hasFocus() && document.activeElement === input;

    if (!focused) {
      if (performance.now() - t0 > FOCUS_TIMEOUT_MS) {
        console.warn('[quick] 等待焦点超时，放弃本次测量');
        return;
      }
      // 焦点没来就反复重新申请。WebView2 在窗口 hide/show 后
      // 偶尔会丢掉焦点请求，重试比等它自己好。
      input.focus();
      requestAnimationFrame(tick);
      return;
    }

    void (async () => {
      const ms = await invoke<number | null>('mark_ready', { frames });
      latency.textContent = ms === null ? '' : `${ms} ms`;
      latency.dataset.slow = String(ms !== null && ms > 200);
    })();
  };

  requestAnimationFrame(tick);
}

listen('quick:show', () => {
  input.value = '';
  latency.textContent = '';
  assist.hide(); // 关闭可能残留的弹层
  input.focus();
  awaitTypable();
});

input.addEventListener('keydown', async (e) => {
  // Esc 优先级最高：关闭弹层或关闭速记条（spec todo/input-assist）
  if (e.key === 'Escape') {
    // 如果弹层开着，Esc 只关弹层，不关速记条
    if (document.querySelector('.assist-layer')) {
      assist.hide();
      e.preventDefault();
      return;
    }
    // 弹层没开，关闭速记条
    input.value = '';
    await invoke('hide_quick');
    return;
  }

  // 弹层开着时，上下键和回车交给 assist 模块处理（已在 assist.ts 注册全局监听）
  if (document.querySelector('.assist-layer')) {
    if (e.key === 'ArrowDown' || e.key === 'ArrowUp' || e.key === 'Enter') {
      // assist.ts 会 preventDefault，这里不用管
      return;
    }
  }

  // Enter 提交
  if (e.key === 'Enter' && !e.isComposing) {
    const text = input.value.trim();
    if (text) {
      try {
        await invoke('capture', { text });
      } catch (err) {
        // vault 不可用等情况：保留用户刚敲的内容，不清空、不关窗。
        latency.textContent = String(err);
        latency.dataset.slow = 'true';
        return;
      }
    }
    input.value = '';
    assist.hide();
    await invoke('hide_quick');
  }
});

// 触发字符集合（D2）
const TRIGGER_CHARS = new Set(['@', '!', '#', '^']);

// input 事件：检查是否需要显示弹层
input.addEventListener('input', () => {
  const text = input.value;
  const cursor = input.selectionStart ?? text.length;

  // 弹层已开：更新过滤词
  if (document.querySelector('.assist-layer')) {
    const filter = extractFilter(text, cursor);
    assist.updateFilter(filter);
    return;
  }

  // 弹层未开：检查光标左侧是否有触发字符
  if (cursor > 0 && TRIGGER_CHARS.has(text[cursor - 1])) {
    requestParse(text, cursor);
  }
});

// 可弹层的标记种类（~id 由系统分配，不给用户选）
const ASSISTABLE = new Set(['time', 'repeat', 'tag', 'intensity']);

/**
 * 请求解析（带序号，丢弃过期响应）。
 *
 * 光标处的东西可能落在两个地方：
 *   - markers：值已合法（`@2026-08-15`），用户可能想改
 *   - degraded：值还不合法（刚敲下的 `@`、半个标签 `#工`），这才是最常见的
 *     弹层时机——所以两处都要查，只查 markers 等于永远不弹。
 *
 * span 是 UTF-8 字节偏移，JS 字符串是 UTF-16，中文场景下两者不等，
 * 替换范围必须在字节域里算（见 sliceByBytes）。
 */
async function requestParse(text: string, cursor: number): Promise<void> {
  const seq = ++parseSeq;

  let result: TodoLine | null;
  try {
    // bare: 速记条输入没有 `- [ ] ` 前缀
    result = await invoke('parse_todo_line', { text, bare: true });
  } catch (err) {
    // 解析不可用不能影响打字，静默放弃本次弹层
    console.error('解析失败:', err);
    return;
  }
  if (parseSeq !== seq || !result) return; // 过期响应，丢弃

  const cursorByte = utf16ToByte(text, cursor);

  // 先找合法标记，再找降级 token（后者是打字中间态，命中率更高）
  const hit =
    result.markers.find(
      (m) => cursorByte >= m.span.start && cursorByte <= m.span.end
    ) ??
    result.degraded.find(
      (d) => cursorByte >= d.span.start && cursorByte <= d.span.end
    );
  if (!hit) return;

  const kind = 'value' in hit ? hit.value.kind : hit.suspected;
  if (!ASSISTABLE.has(kind)) return;

  const rect = input.getBoundingClientRect();
  const filter = extractFilter(text, cursor);

  assist.show(
    kind as 'time' | 'repeat' | 'tag' | 'intensity',
    rect.left,
    rect.bottom,
    filter,
    (value) => {
      // 用解析器给的 span 替换，而不是自己找边界——边界规则只有 Rust 知道
      const before = sliceByBytes(text, 0, hit.span.start);
      const after = sliceByBytes(text, hit.span.end, null);
      input.value = before + value + after;
      const caret = (before + value).length;
      input.setSelectionRange(caret, caret);
      input.focus();
    }
  );
}

/** UTF-16 下标 → UTF-8 字节偏移 */
function utf16ToByte(text: string, idx: number): number {
  return new TextEncoder().encode(text.slice(0, idx)).length;
}

/** 按字节偏移切片，返回 JS 字符串 */
function sliceByBytes(text: string, from: number, to: number | null): string {
  const bytes = new TextEncoder().encode(text);
  const part = to === null ? bytes.slice(from) : bytes.slice(from, to);
  return new TextDecoder().decode(part);
}

// 提取过滤词：从 marker 起始位置到光标之间的文本（去掉前导标记字符）
function extractFilter(text: string, cursor: number): string {
  // 简化实现：从光标向左找第一个触发字符
  let start = cursor - 1;
  while (start >= 0 && !TRIGGER_CHARS.has(text[start])) {
    start--;
  }
  if (start < 0) return '';
  return text.slice(start + 1, cursor).trim();
}

// 通知 Rust 侧：预热窗口的前端已就绪（WebView 已完成首帧）
invoke('quick_warmed');
