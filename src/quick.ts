import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import type { TodoLine } from './types/todo';

const input = document.querySelector<HTMLInputElement>('#input')!;
const latency = document.querySelector<HTMLSpanElement>('#latency')!;
const timeBtn = document.querySelector<HTMLButtonElement>('#time-btn')!;

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
  input.focus();
  awaitTypable();
});

/**
 * 时间选择器返回结果：一条待办只能有一个时间，所以已有时间标记时必须
 * **替换**它，而不是在光标处再插一个（那样会攒出一串时间，解析器只认第一个，
 * 其余全被判为重复而降级）。
 *
 * 目标位置由 Rust 解析器给出，前端不自己找边界——边界规则（引号屏蔽、
 * 右向左扫描、两 token 时间）只有解析器知道。
 */
listen<string>('time-picker:selected', async (event) => {
  const value = event.payload;
  const text = input.value;

  let span: { start: number; end: number } | null = null;
  try {
    const parsed: TodoLine | null = await invoke('parse_todo_line', {
      text,
      bare: true,
    });
    // markers 里的合法时间，或 degraded 里的半成品时间，都算已有的时间标记
    span =
      parsed?.markers.find((m) => m.value.kind === 'time')?.span ??
      parsed?.degraded.find((d) => d.suspected === 'time')?.span ??
      null;
  } catch (err) {
    // 解析不可用时退化为「插到末尾」，至少不丢用户选的时间
    console.error('解析失败，退化为追加:', err);
  }

  if (span) {
    // 替换已有时间标记（span 是 UTF-8 字节偏移，闭区间）
    const before = sliceByBytes(text, 0, span.start);
    const after = sliceByBytes(text, span.end + 1, null);
    input.value = before + value + after;
    const caret = (before + value).length;
    input.setSelectionRange(caret, caret);
  } else {
    // 没有时间标记：追加到末尾，与正文之间留一个空格
    const base = text.trimEnd();
    input.value = base.length > 0 ? `${base} ${value}` : value;
    input.setSelectionRange(input.value.length, input.value.length);
  }

  input.focus();
});

/** 按 UTF-8 字节偏移切片。to 为 null 表示到结尾 */
function sliceByBytes(text: string, from: number, to: number | null): string {
  const bytes = new TextEncoder().encode(text);
  const part = to === null ? bytes.slice(from) : bytes.slice(from, to);
  return new TextDecoder().decode(part);
}

// 时间按钮
timeBtn.addEventListener('click', async () => {
  try {
    await invoke('open_time_picker');
  } catch (err) {
    console.error('打开时间选择器失败:', err);
  }
});

input.addEventListener('keydown', async (e) => {
  // Esc 关闭速记条
  if (e.key === 'Escape') {
    input.value = '';
    await invoke('hide_quick');
    return;
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
    await invoke('hide_quick');
  }
});

// 通知 Rust 侧：预热窗口的前端已就绪（WebView 已完成首帧）
invoke('quick_warmed');
