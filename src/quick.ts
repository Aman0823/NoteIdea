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

// 请求解析（带序号，丢弃过期响应）
async function requestParse(text: string, cursor: number): Promise<void> {
  const seq = ++parseSeq;

  try {
    const result: TodoLine | null = await invoke('parse_todo_line', { text });
    if (parseSeq !== seq) return; // 过期响应，丢弃

    if (!result) return; // 不是待办行

    // 找到光标所在的 marker
    const marker = result.markers.find(m => cursor >= m.span.start && cursor <= m.span.end);
    if (!marker) return;

    const kind = marker.value.kind;
    if (kind !== 'time' && kind !== 'repeat' && kind !== 'tag' && kind !== 'intensity') {
      return; // ~id 不弹层
    }

    // 计算弹层锚点（输入框左下角）
    const rect = input.getBoundingClientRect();
    const anchorX = rect.left;
    const anchorY = rect.bottom;

    const filter = extractFilter(text, cursor);

    assist.show(kind, anchorX, anchorY, filter, (value) => {
      // 确认回调：替换当前 marker 为选中的值
      const before = text.slice(0, marker.span.start);
      const after = text.slice(marker.span.end);
      input.value = before + value + after;
      input.setSelectionRange(before.length + value.length, before.length + value.length);
      input.focus();
    });
  } catch (err) {
    console.error('解析失败:', err);
  }
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
