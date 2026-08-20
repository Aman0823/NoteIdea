import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import type { MarkerValue } from './types/todo';

/** 本窗口发起的选择器请求 id。结果是广播，靠它区分是不是自己要的。 */
let pickerRequestId = '';

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
listen<{ requestId: string; value: MarkerValue }>('marker-picker:selected', async (event) => {
  // 结果是广播，主编辑器点 chip 时速记条也收得到。只认自己发起的那一次。
  if (event.payload.requestId !== pickerRequestId) return;
  pickerRequestId = '';

  try {
    // 规范文本与「替换还是追加」都交给 Rust：边界规则（引号屏蔽、右向左
    // 扫描、两 token 时间）只有解析器知道，前端不自己找也不自己拼。
    input.value = await invoke<string>('write_marker', {
      line: input.value,
      value: event.payload.value,
    });
    input.setSelectionRange(input.value.length, input.value.length);
  } catch (err) {
    console.error('写入标记失败:', err);
  }

  input.focus();
});

// 时间按钮
timeBtn.addEventListener('click', async () => {
  try {
    pickerRequestId = `quick-${Date.now()}`;
    await invoke('open_marker_picker', { kind: 'time', requestId: pickerRequestId });
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
