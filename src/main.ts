import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import { open } from '@tauri-apps/plugin-dialog';

const statsEl = document.querySelector<HTMLDivElement>('#stats')!;
const samplesEl = document.querySelector<HTMLUListElement>('#samples')!;
const warningsEl = document.querySelector<HTMLDivElement>('#warnings')!;

function percentile(sorted: number[], p: number): number {
  if (sorted.length === 0) return 0;
  const idx = Math.min(sorted.length - 1, Math.ceil((p / 100) * sorted.length) - 1);
  return sorted[Math.max(0, idx)];
}

async function refresh() {
  const samples = await invoke<number[]>('timings');

  if (samples.length === 0) {
    statsEl.textContent = '尚无数据。先按 Alt+Space 试几次。';
    samplesEl.replaceChildren();
    return;
  }

  const sorted = [...samples].sort((a, b) => a - b);
  const avg = samples.reduce((a, b) => a + b, 0) / samples.length;
  const p95 = percentile(sorted, 95);
  const pass = p95 <= 200;

  statsEl.replaceChildren();
  const rows: [string, string][] = [
    ['样本数', String(samples.length)],
    ['首次（冷）', `${samples[0]} ms`],
    ['最小', `${sorted[0]} ms`],
    ['中位', `${percentile(sorted, 50)} ms`],
    ['平均', `${avg.toFixed(1)} ms`],
    ['p95', `${p95} ms`],
    ['最大', `${sorted[sorted.length - 1]} ms`],
  ];
  for (const [k, v] of rows) {
    const row = document.createElement('div');
    row.className = 'stat';
    const key = document.createElement('span');
    key.textContent = k;
    const val = document.createElement('strong');
    val.textContent = v;
    row.append(key, val);
    statsEl.append(row);
  }

  const verdict = document.createElement('div');
  verdict.className = pass ? 'verdict pass' : 'verdict fail';
  verdict.textContent = pass
    ? `p95 ${p95}ms ≤ 200ms —— NFR-2 达标，D22 的预热方案成立`
    : `p95 ${p95}ms > 200ms —— NFR-2 未达标，预热方案需要重新设计`;
  statsEl.append(verdict);

  samplesEl.replaceChildren();
  for (const [i, ms] of samples.entries()) {
    const li = document.createElement('li');
    li.textContent = `#${i + 1}　${ms} ms`;
    li.dataset.slow = String(ms > 200);
    samplesEl.append(li);
  }
}

document.querySelector<HTMLButtonElement>('#refresh')!.addEventListener('click', refresh);
listen('timings:changed', refresh);

function renderHotkeyFailures(failed: string[]) {
  warningsEl.replaceChildren();
  if (failed.length === 0) return;
  const box = document.createElement('div');
  box.className = 'warn';
  box.textContent = `热键注册失败：${failed.join('、')}。可能被其他软件占用（FR-21 要求提示到具体是哪个键）。`;
  warningsEl.append(box);
}

// setup 阶段发出的事件早于这里 listen 完成，会丢；所以主动拉一次现状。
// 事件监听只为覆盖后续动态改键的情况。
listen<string[]>('hotkey:failed', (e) => renderHotkeyFailures(e.payload));
invoke<string[]>('hotkey_failures').then(renderHotkeyFailures);

// ---------- vault 选择（degraded 状态） ----------

const vaultCard = document.querySelector<HTMLElement>('#vault-card')!;
const vaultReason = document.querySelector<HTMLParagraphElement>('#vault-reason')!;

async function refreshVault() {
  const reason = await invoke<string | null>('vault_state');
  // reason 为 null 说明 vault 可用，此时不打扰用户。
  vaultCard.classList.toggle('hidden', reason === null);
  if (reason !== null) vaultReason.textContent = `${reason}。速记和笔记功能需要先指定一个文件夹。`;
}

document.querySelector<HTMLButtonElement>('#choose-vault')!.addEventListener('click', async () => {
  const picked = await open({ directory: true, multiple: false, title: '选择笔记存放文件夹' });
  if (typeof picked !== 'string') return; // 用户取消
  try {
    await invoke('choose_vault', { path: picked });
  } catch (e) {
    vaultReason.textContent = `选择失败：${String(e)}`;
    return;
  }
  await refreshVault();
});

listen('vault:changed', refreshVault);

// 写入失败（重试耗尽）。actor 走的是异步落盘，失败必须让用户看到，
// 否则会以为记下来了而实际没有。
listen<{ file: string; op: string; error: string }>('write:failed', (e) => {
  const box = document.createElement('div');
  box.className = 'warn';
  box.textContent = `写入 ${e.payload.file} 失败：${e.payload.error}`;
  warningsEl.append(box);
});

refreshVault();
refresh();
