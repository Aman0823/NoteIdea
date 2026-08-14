import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';

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

// 时间选择器返回结果
listen<string>('time-picker:selected', (event) => {
  const value = event.payload;
  const cursor = input.selectionStart ?? input.value.length;
  const before = input.value.slice(0, cursor);
  const after = input.value.slice(cursor);
  input.value = before + value + after;
  const newCursor = (before + value).length;
  input.setSelectionRange(newCursor, newCursor);
  input.focus();
});

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
