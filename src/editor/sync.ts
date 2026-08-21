// 落盘广播 → 本文档坐标系的变更（design E3）
//
// actor 是唯一写者，落盘成功后广播它做了什么。本窗口收到后有两种可能：
//   1. 就是自己刚提交的那一批 → 标记已确认，绝不再应用一遍
//   2. 别的写者干的（速记条往 inbox.md 追加、将来的便签勾选）→ 作为远端变更
//      应用进缓冲，并把本地未确认的变更在新基线上重映射
//
// 远端变更带 annotation 且 addToHistory: false：D20 要求每个窗口只能撤销
// 自己产生的变更。随手 Ctrl+Z 撤掉十秒前便签上那下勾选，行为莫名且极难排查。

import { Annotation, ChangeSet, Transaction, type ChangeSpec } from '@codemirror/state';
import type { EditorView } from '@codemirror/view';

/** 标记这次事务来自别的写者，不是本窗口的编辑。 */
export const remoteChange = Annotation.define<boolean>();

/** actor 广播的 op（与 Rust 的 Operation 一一对应）。 */
export type RustOperation =
  | { kind: 'append'; content: string }
  | { kind: 'replace_line'; line_number: number; old_content: string; new_content: string }
  | { kind: 'create'; content: string }
  | { kind: 'apply_edits'; edits: Array<{ from: number; to: number; insert: string }> }
  | { kind: 'replace_file'; content: string };

export interface RustChangeSet {
  file_path: string;
  op: RustOperation;
  base_hash: string | null;
}

export interface FileChangedEvent {
  id: number;
  file: string;
  op: string;
  changeSet: RustChangeSet;
}

export interface WriteFailedEvent {
  id: number;
  file: string;
  op: string;
  error: string;
}

/**
 * 按内容定位目标行，返回它在文档中的区间。
 *
 * 与 actor 的 `locate_by_content` 同一个判据：**匹配到多行就放弃**。
 * 改错行是静默的数据损坏，而放弃是可见的、能处理的。
 */
function locateLine(view: EditorView, lineNumber: number, oldContent: string) {
  const doc = view.state.doc;

  // 先看行号指的那行对不对——对得上就不必全文搜。
  if (lineNumber >= 1 && lineNumber <= doc.lines) {
    const line = doc.line(lineNumber);
    if (line.text === oldContent) return line;
  }

  let found: { from: number; to: number } | null = null;
  for (let i = 1; i <= doc.lines; i += 1) {
    const line = doc.line(i);
    if (line.text !== oldContent) continue;
    if (found !== null) return null; // 多行同内容，无法确定改哪一行
    found = { from: line.from, to: line.to };
  }
  return found;
}

/**
 * 把 Rust op 换算成本文档坐标系下的变更。
 *
 * 返回 null 表示「这次广播没法安全地映射到本文档」——宁可不动，也不猜着改。
 */
export function toLocalChange(view: EditorView, op: RustOperation): ChangeSpec | null {
  const doc = view.state.doc;

  switch (op.kind) {
    case 'append': {
      // 与 Rust 的 append 对齐：上一行没有结尾换行时要补一个，否则两行会黏成一行。
      const needsNewline = doc.length > 0 && doc.sliceString(doc.length - 1, doc.length) !== '\n';
      const text = (needsNewline ? '\n' : '') + op.content + '\n';
      return { from: doc.length, to: doc.length, insert: text };
    }

    case 'replace_line': {
      const target = locateLine(view, op.line_number, op.old_content);
      if (target === null) return null;
      return { from: target.from, to: target.to, insert: op.new_content };
    }

    case 'replace_file':
      // 版本恢复：语义本就是整体回退
      return { from: 0, to: doc.length, insert: op.content };

    case 'apply_edits': {
      // 偏移相对提交方的基线。apply_edits 要求基线严格相等才落盘，所以它成功时
      // 磁盘内容 == 提交方基线；只有本文档也等于那份内容，偏移才对得上。
      // 对不上就返回 null，交给调用方走「整篇重载」的兜底。
      if (op.edits.some((e) => e.to > doc.length || e.from > e.to)) return null;
      return op.edits.map((e) => ({ from: e.from, to: e.to, insert: e.insert }));
    }

    case 'create':
      // 新建的是别的文件；本文档没有对应变更
      return null;

    default:
      return null;
  }
}

/**
 * 把远端变更应用进缓冲，并返回它对应的 ChangeSet（供未确认变更重映射）。
 *
 * 不进撤销栈（D20）。
 */
export function applyRemote(view: EditorView, spec: ChangeSpec): ChangeSet {
  const changes = view.state.changes(spec);
  view.dispatch({
    changes,
    annotations: [
      remoteChange.of(true),
      // D20：远端变更不进本窗口撤销栈。少了这一条，Ctrl+Z 会撤掉别人的改动。
      Transaction.addToHistory.of(false),
      Transaction.remote.of(true),
    ],
  });
  return changes;
}
