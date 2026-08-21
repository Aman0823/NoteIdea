// 输入辅助在 CM6 侧的适配层（组 7）
//
// assist.ts 是宿主无关的：它只管显示候选、键盘导航、把选中的结构化值交回来。
// 这个文件负责把它接到编辑器上——判断该不该弹、锚点在哪、选完往哪写。
//
// 7.4 的硬要求：**按键处理绝不 await 解析**。所以触发判定完全是本地的字符
// 检查，一次 invoke 都不发；解析只在用户真的选中之后才发生。

import { invoke } from '@tauri-apps/api/core';
import { EditorView } from '@codemirror/view';
import type { MarkerValue } from '../types/todo';
import { hide, isOpen, show, updateFilter, type AssistKind } from '../assist';
import { applyNewLineText } from './decorations';

/** 触发字符 → 弹层类型。与速记条共用同一张表，不另抄一份。 */
const TRIGGERS: Record<string, AssistKind> = {
  '@': 'time',
  '!': 'repeat',
  '#': 'tag',
  '^': 'intensity',
};

/** 弹层打开时，触发字符在文档中的位置。 */
let triggerPos: number | null = null;

function closeLayer() {
  triggerPos = null;
  hide();
}

/** 把选中的标记值写回当前行。文本由 Rust 产出，前端不拼格式。 */
async function insertMarker(view: EditorView, anchor: number, value: MarkerValue) {
  // 光标位置必须在关层之前读：关层会让编辑器重新拿回焦点，晚读可能已经变了。
  const cursor = view.state.selection.main.head;
  closeLayer();

  if (anchor > view.state.doc.length) return;
  const line = view.state.doc.lineAt(anchor);

  // 先把用户敲的触发字符和后面的过滤词删掉——那是「正在输入的半成品」，
  // write_marker 要在一个干净的行上决定替换还是追加。
  const draftEnd = Math.max(anchor, Math.min(cursor, line.to));
  const cleaned =
    line.text.slice(0, anchor - line.from) + line.text.slice(draftEnd - line.from);

  try {
    const next = await invoke<string>('write_marker', { line: cleaned, value });
    applyNewLineText(view, line.from, line.text, next);
  } catch (err) {
    console.error('插入标记失败:', err);
  }
}

/** 光标的屏幕坐标，作为弹层锚点。 */
function anchorAt(view: EditorView, pos: number): { x: number; y: number } | null {
  const rect = view.coordsAtPos(pos);
  if (rect === null) return null;
  return { x: rect.left, y: rect.bottom };
}

function openLayer(view: EditorView, kind: AssistKind, pos: number) {
  const anchor = anchorAt(view, pos);
  if (anchor === null) return;

  triggerPos = pos;
  show(kind, '', {
    anchor,
    // pos 走闭包捕获，不读模块变量：assist 关层时会先触发 onClose 把
    // triggerPos 清掉，再回调 onPick——依赖模块变量的话这里永远读到 null。
    onPick: (value) => void insertMarker(view, pos, value),
    onClose: () => {
      triggerPos = null;
    },
  });
}

/**
 * 触发判定。只看本地字符，不发任何请求（7.4）。
 *
 * 只在触发字符前面是行首或空白时才弹——否则 `zhang@corp.com` 里的 `@`、
 * `C#` 里的 `#` 都会莫名其妙弹出选择层。这条规则与解析器「元数据区从右
 * 向左扫描」的判据一致：粘在词中间的标记字符本来就不是标记。
 */
function shouldTrigger(view: EditorView, pos: number): boolean {
  if (pos <= 0) return true;
  const before = view.state.doc.sliceString(pos - 1, pos);
  return before === ' ' || before === '\t';
}

export const assistInput = [
  EditorView.updateListener.of((update) => {
    if (!update.docChanged) {
      // 光标跑到别处（点击、方向键）就收层，别让它悬在无关的位置上
      if (isOpen() && update.selectionSet && triggerPos !== null) {
        const head = update.state.selection.main.head;
        const line = update.state.doc.lineAt(triggerPos);
        if (head < triggerPos || head > line.to) closeLayer();
      }
      return;
    }

    const view = update.view;

    // 弹层已开：把触发字符之后的内容当作过滤词
    if (isOpen() && triggerPos !== null) {
      const head = update.state.selection.main.head;
      if (head <= triggerPos) {
        // 触发字符被删掉了
        closeLayer();
        return;
      }
      updateFilter(update.state.doc.sliceString(triggerPos + 1, head));
      return;
    }

    // 弹层未开：看这次变更是不是敲入了触发字符
    let opened = false;
    update.changes.iterChanges((_fromA, _toA, _fromB, toB, inserted) => {
      if (opened || inserted.length !== 1) return;
      const ch = inserted.toString();
      const kind = TRIGGERS[ch];
      if (kind === undefined) return;

      const triggerAt = toB - 1;
      if (!shouldTrigger(view, triggerAt)) return;

      opened = true;
      // 等这一轮 update 走完再弹：正在 update 里读坐标拿到的是旧布局
      queueMicrotask(() => openLayer(view, kind, triggerAt));
    });
  }),

  // 失焦收层：弹层是 body 上的浮动元素，编辑器没了焦点它不该还悬着
  EditorView.domEventHandlers({
    blur() {
      if (isOpen()) closeLayer();
      return false;
    },
  }),
];
