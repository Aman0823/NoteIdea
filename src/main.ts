import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import { open } from '@tauri-apps/plugin-dialog';
import { ChangeSet } from '@codemirror/state';
import { createNoteEditor, type NoteEditor } from './editor/editor';
import { primeParseCache } from './editor/decorations';

const diagnosticsEl = document.querySelector<HTMLElement>('#diagnostics')!;
document.querySelector<HTMLButtonElement>('#diagnostics-toggle')!.addEventListener('click', () => {
  diagnosticsEl.classList.toggle('hidden');
});

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
const welcomeCard = document.querySelector<HTMLElement>('#welcome-card')!;
const workspaceLabel = document.querySelector<HTMLParagraphElement>('#workspace-label')!;

const workspaceEl = document.querySelector<HTMLElement>('#editor-workspace')!;

function clearWorkspaceView() {
  currentPath = null;
  workspaceEl.classList.add('hidden');
  noteTitleEl.textContent = '选择一篇笔记';
  notePathEl.textContent = '';
  editorEmptyEl.classList.remove('hidden');
  resetEditor();
}

interface WorkspaceState {
  ready: boolean;
  chosenBefore: boolean;
  path: string | null;
  reason: string | null;
}

async function refreshVault() {
  const state = await invoke<WorkspaceState>('vault_state');
  if (!state.ready) {
    clearWorkspaceView();
    // 首次从未选过：正常欢迎态；已保存路径失效：可见故障提示。
    welcomeCard.classList.toggle('hidden', state.chosenBefore);
    vaultCard.classList.toggle('hidden', !state.chosenBefore);
    if (state.chosenBefore) {
      vaultReason.textContent = `${state.reason ?? '工作区不可用'}。请选择一个可用的工作区。`;
      workspaceLabel.textContent = '工作区不可用';
    } else {
      workspaceLabel.textContent = '本地 Markdown 笔记';
    }
    return;
  }

  vaultCard.classList.add('hidden');
  welcomeCard.classList.add('hidden');
  workspaceEl.classList.remove('hidden');
  workspaceLabel.textContent = state.path ?? '已恢复上次工作区';
  await refreshTree();
}

async function chooseWorkspace() {
  const picked = await open({ directory: true, multiple: false, title: '选择笔记工作区' });
  if (typeof picked !== 'string') return;
  // 切换工作区前先落盘当前缓冲；失败则不切走，避免旧工作区内容丢失。
  if (!(await flush())) return;
  try {
    await invoke('choose_vault', { path: picked });
  } catch (e) {
    clearWorkspaceView();
    welcomeCard.classList.add('hidden');
    vaultCard.classList.remove('hidden');
    vaultReason.textContent = `选择失败：${String(e)}`;
    return;
  }
  // choose_vault 会发 vault:changed，refreshVault 会据此刷新并加载文件树；
  // 这里只清理旧视图，不重复刷树。
  clearWorkspaceView();
}

document.querySelector<HTMLButtonElement>('#choose-vault')!.addEventListener('click', chooseWorkspace);
document.querySelector<HTMLButtonElement>('#welcome-choose')!.addEventListener('click', chooseWorkspace);

listen('vault:changed', refreshVault);

// ---------- 写入失败列表 ----------

const failedCard = document.querySelector<HTMLElement>('#failed-card')!;
const failedList = document.querySelector<HTMLUListElement>('#failed-list')!;

interface FailedWrite {
  id: number;
  file: string;
  op: string;
  error: string;
  at: number;
}

const OP_LABEL: Record<string, string> = {
  append: '追加',
  replace_line: '改行',
  create: '新建',
  replace_file: '整篇替换',
};

async function refreshFailed() {
  const rows = await invoke<FailedWrite[]>('failed_writes');
  failedCard.classList.toggle('hidden', rows.length === 0);
  failedList.replaceChildren();

  for (const row of rows) {
    const li = document.createElement('li');

    const head = document.createElement('div');
    head.className = 'failed-head';
    const file = document.createElement('strong');
    file.textContent = row.file;
    const op = document.createElement('span');
    op.className = 'failed-op';
    op.textContent = OP_LABEL[row.op] ?? row.op;
    const when = document.createElement('span');
    when.className = 'failed-op';
    when.textContent = new Date(row.at).toLocaleString('zh-CN');
    head.append(file, op, when);

    const why = document.createElement('div');
    why.className = 'failed-why';
    why.textContent = row.error;

    const actions = document.createElement('div');
    actions.className = 'failed-actions';
    const retry = document.createElement('button');
    retry.type = 'button';
    retry.textContent = '重试';
    const discard = document.createElement('button');
    discard.type = 'button';
    discard.textContent = '丢弃';

    for (const [btn, cmd] of [
      [retry, 'retry_write'],
      [discard, 'discard_write'],
    ] as const) {
      btn.addEventListener('click', async () => {
        retry.disabled = true;
        discard.disabled = true;
        try {
          await invoke(cmd, { id: row.id });
        } catch (e) {
          why.textContent = `操作失败：${String(e)}`;
          retry.disabled = false;
          discard.disabled = false;
          return;
        }
        await refreshFailed();
      });
    }

    actions.append(retry, discard);
    li.append(head, why, actions);
    failedList.append(li);
  }
}

// actor 走异步落盘，失败必须让用户看到，否则会以为记下来了而实际没有。
listen('write:failed', refreshFailed);
// 重试成功后那条会从失败列表消失。
listen('file:changed', refreshFailed);

// ---------- 文件树与笔记打开 ----------

interface NoteTreeNode {
  name: string;
  path: string;
  kind: 'directory' | 'file';
  children: NoteTreeNode[];
  error: string | null;
}

interface NoteContent {
  path: string;
  content: string;
  hash: string;
}

const treeEl = document.querySelector<HTMLElement>('#note-tree')!;
const treeStatusEl = document.querySelector<HTMLDivElement>('#tree-status')!;
const noteTitleEl = document.querySelector<HTMLSpanElement>('#note-title')!;
const notePathEl = document.querySelector<HTMLSpanElement>('#note-path')!;
const editorHostEl = document.querySelector<HTMLDivElement>('#note-editor')!;
const editorEmptyEl = document.querySelector<HTMLDivElement>('#editor-empty')!;

let currentPath: string | null = null;

// ---------- 编辑器与自动保存 ----------

let editor: NoteEditor | null = null;
let savedHash = '';
// 自上次落盘基线以来的未提交变更（相对 savedHash 所指内容的字符偏移）。
let unconfirmed: ChangeSet | null = null;
let autosaveTimer: number | null = null;
let flushing = false;

function resetEditor() {
  if (autosaveTimer !== null) {
    clearTimeout(autosaveTimer);
    autosaveTimer = null;
  }
  if (editor !== null) {
    editor.destroy();
    editor = null;
  }
  unconfirmed = null;
  savedHash = '';
  flushing = false;
  editorHostEl.classList.add('hidden');
}

function showEditor() {
  editorHostEl.classList.remove('hidden');
  editorEmptyEl.classList.add('hidden');
}

function scheduleAutosave() {
  if (autosaveTimer !== null) clearTimeout(autosaveTimer);
  autosaveTimer = window.setTimeout(() => {
    autosaveTimer = null;
    void flush();
  }, 800);
}

async function flush(): Promise<boolean> {
  if (flushing) return true;
  if (editor === null || currentPath === null || unconfirmed === null) return true;
  // 无变更不提交空 ChangeSet。
  if (unconfirmed.empty) return true;

  // 快照本次要提交的变更与提交时的基线内容，并立即把 unconfirmed 重置为
  // 相对「提交后新基线」的空变更。这样 flush 期间用户继续输入的内容会累积到
  // 正确的新坐标系，而不是被乐观更新吞掉。
  const submitted = unconfirmed;
  const baseContent = editor.getContent();
  const edits: { from: number; to: number; insert: string }[] = [];
  submitted.iterChanges((fromA, toA, _fromB, _toB, inserted) => {
    edits.push({ from: fromA, to: toA, insert: inserted.toString() });
  });
  unconfirmed = ChangeSet.empty(editor.view.state.doc.length);

  flushing = true;
  try {
    await invoke('apply_edits', {
      filePath: currentPath,
      baseHash: savedHash,
      edits,
    });
    // 落盘基线推进到提交时那份内容（baseContent），而非此刻可能又变了的缓冲。
    savedHash = await invoke<string>('hash_content', { content: baseContent });
    return true;
  } catch (e) {
    // 入队失败：把快照合并回 unconfirmed，用户已敲的内容不丢。
    unconfirmed = submitted.compose(unconfirmed);
    showSaveError(String(e));
    return false;
  } finally {
    flushing = false;
    if (unconfirmed !== null && !unconfirmed.empty) scheduleAutosave();
  }
}

function showSaveError(message: string) {
  // 复用诊断面板的失败可见通道：不静默吞掉落盘失败。
  const box = document.createElement('div');
  box.className = 'warn';
  box.textContent = `保存失败：${message}`;
  warningsEl.append(box);
}

// 窗口失焦或隐藏（主窗口关闭即隐藏）时立即落盘，不等 debounce。
window.addEventListener('blur', () => {
  void flush();
});

function setTreeStatus(message: string | null) {
  treeStatusEl.textContent = message ?? '';
  treeStatusEl.classList.toggle('hidden', message === null);
}

function renderNodes(nodes: NoteTreeNode[], container: HTMLElement) {
  for (const node of nodes) {
    if (node.kind === 'file') {
      const button = document.createElement('button');
      button.type = 'button';
      button.className = 'tree-node';
      button.dataset.path = node.path;
      const icon = document.createElement('span');
      icon.className = 'tree-twisty';
      const label = document.createElement('span');
      label.className = 'tree-label';
      label.textContent = node.name;
      button.append(icon, label);
      button.addEventListener('click', () => openNote(node.path));
      container.append(button);
      continue;
    }

    const wrapper = document.createElement('div');
    const toggle = document.createElement('button');
    toggle.type = 'button';
    toggle.className = 'tree-node';
    const twisty = document.createElement('span');
    twisty.className = 'tree-twisty';
    twisty.textContent = '▾';
    const label = document.createElement('span');
    label.className = 'tree-label';
    label.textContent = node.name;
    toggle.append(twisty, label);

    const children = document.createElement('div');
    children.className = 'tree-children';
    if (node.error !== null) {
      const error = document.createElement('div');
      error.className = 'tree-error';
      error.textContent = node.error;
      children.append(error);
    }
    renderNodes(node.children, children);

    toggle.addEventListener('click', () => {
      const collapsed = children.classList.toggle('hidden');
      twisty.textContent = collapsed ? '▸' : '▾';
    });

    wrapper.append(toggle, children);
    container.append(wrapper);
  }
}

function markActive(path: string | null) {
  for (const node of treeEl.querySelectorAll<HTMLButtonElement>('.tree-node[data-path]')) {
    node.setAttribute('aria-current', String(node.dataset.path === path));
  }
}

async function refreshTree() {
  setTreeStatus('正在读取笔记...');
  let tree: NoteTreeNode;
  try {
    tree = await invoke<NoteTreeNode>('list_notes');
  } catch (e) {
    treeEl.replaceChildren();
    setTreeStatus(`读取笔记失败：${String(e)}`);
    return;
  }

  treeEl.replaceChildren();
  renderNodes(tree.children, treeEl);
  setTreeStatus(tree.children.length === 0 ? '这个文件夹里还没有 Markdown 笔记。' : null);
  markActive(currentPath);
}

async function openNote(path: string) {
  // 切换前先落盘当前缓冲；落盘失败则留在当前笔记（design E8）。
  if (!(await flush())) return;

  let note: NoteContent;
  try {
    note = await invoke<NoteContent>('read_note', { path });
  } catch (e) {
    // 文件可能已被外部删除：如实报告并刷新，不创建空文件顶替。
    noteTitleEl.textContent = '打开失败';
    notePathEl.textContent = String(e);
    resetEditor();
    editorEmptyEl.classList.remove('hidden');
    currentPath = null;
    await refreshTree();
    return;
  }

  currentPath = note.path;
  noteTitleEl.textContent = note.path.split('/').pop() ?? note.path;
  notePathEl.textContent = note.path;
  savedHash = note.hash;
  unconfirmed = ChangeSet.empty(note.content.length);

  // 打开即批量解析（design E4）：一次 invoke 填满缓存，避免滚动时逐屏补请求。
  // 不 await——解析没回来之前按原文显示即可，不该拖慢打开。
  void primeParseCache(note.content.split('\n'));

  if (editor !== null) editor.destroy();
  editor = createNoteEditor(
    editorHostEl,
    note.content,
    (update) => {
      if (update.docChanged && unconfirmed !== null) {
        unconfirmed = unconfirmed.compose(update.changes);
        scheduleAutosave();
      }
    },
    // 点复选框、改 chip 是明确动作，不等 800ms（design E7）
    () => void flush(),
  );
  showEditor();
  markActive(currentPath);
}

document.querySelector<HTMLButtonElement>('#refresh-notes')!.addEventListener('click', refreshTree);

document.querySelector<HTMLButtonElement>('#new-note')!.addEventListener('click', async () => {
  const name = window.prompt('新笔记名称（可含子目录，例如 工作/周报.md）');
  if (name === null) return;
  const trimmed = name.trim();
  if (trimmed === '') return;
  const filePath = trimmed.toLowerCase().endsWith('.md') ? trimmed : `${trimmed}.md`;

  try {
    await invoke('create', { filePath, content: '' });
  } catch (e) {
    setTreeStatus(`新建失败：${String(e)}`);
    return;
  }
  await refreshTree();
  await openNote(filePath);
});

refreshVault();
refreshFailed();
refresh();
