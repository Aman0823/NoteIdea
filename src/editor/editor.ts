import { EditorState } from '@codemirror/state';
import {
  EditorView,
  keymap,
  lineNumbers,
  drawSelection,
  highlightActiveLine,
  type ViewUpdate,
} from '@codemirror/view';
import { defaultKeymap, history, historyKeymap, indentWithTab } from '@codemirror/commands';
import { markdown, markdownLanguage } from '@codemirror/lang-markdown';
import { syntaxHighlighting, HighlightStyle, indentOnInput } from '@codemirror/language';
import { tags } from '@lezer/highlight';
import { todoChips, markerInteraction, flushRequest } from './decorations';

// 深色主题，与 styles.css 的配色保持一致（--panel / --text / --accent）。
const darkTheme = EditorView.theme(
  {
    '&': { height: '100%', backgroundColor: 'transparent' },
    '.cm-content': {
      caretColor: '#f5c542',
      fontFamily: "'Cascadia Code', Consolas, monospace",
      fontSize: '13px',
      lineHeight: '1.7',
    },
    '.cm-cursor, .cm-dropCursor': { borderLeftColor: '#f5c542' },
    '&.cm-focused > .cm-scroller > .cm-selectionLayer .cm-selectionBackground, .cm-selectionBackground':
      { backgroundColor: 'rgba(245, 197, 66, 0.16)' },
    '.cm-gutters': {
      backgroundColor: 'transparent',
      color: '#6f6885',
      border: 'none',
    },
    '.cm-activeLine': { backgroundColor: 'rgba(245, 197, 66, 0.04)' },
    '.cm-activeLineGutter': { backgroundColor: 'transparent' },
  },
  { dark: true },
);

const mdHighlight = HighlightStyle.define([
  { tag: tags.heading, color: '#f5c542', fontWeight: '600' },
  { tag: tags.emphasis, fontStyle: 'italic' },
  { tag: tags.strong, fontWeight: '700' },
  { tag: tags.monospace, color: '#4ade80' },
  { tag: tags.link, color: '#7aa2f7' },
  { tag: tags.url, color: '#7aa2f7' },
  { tag: tags.comment, color: '#6f6885' },
  { tag: tags.quote, color: '#9a94ad', fontStyle: 'italic' },
  { tag: tags.processingInstruction, color: '#9a94ad' },
  { tag: tags.meta, color: '#9a94ad' },
]);

export interface NoteEditor {
  view: EditorView;
  getContent(): string;
  setContent(content: string): void;
  focus(): void;
  destroy(): void;
}

export function createNoteEditor(
  parent: HTMLElement,
  initialContent: string,
  onChange: (update: ViewUpdate) => void,
  onFlushRequest: () => void = () => {},
): NoteEditor {
  const extensions = [
    lineNumbers(),
    history(),
    drawSelection(),
    highlightActiveLine(),
    keymap.of([...defaultKeymap, ...historyKeymap, indentWithTab]),
    // GFM base：任务列表、删除线、表格都要它，commonmark 默认不带。
    markdown({ base: markdownLanguage }),
    indentOnInput(),
    syntaxHighlighting(mdHighlight),
    todoChips,
    markerInteraction,
    flushRequest.of(onFlushRequest),
    darkTheme,
    EditorView.lineWrapping,
    EditorView.updateListener.of(onChange),
  ];

  const view = new EditorView({
    state: EditorState.create({ doc: initialContent, extensions }),
    parent,
  });

  return {
    view,
    getContent() {
      return view.state.doc.toString();
    },
    setContent(content) {
      // 用 setState 整体替换：不产生 docChanged 事务，避免被当成一次编辑提交。
      view.setState(EditorState.create({ doc: content, extensions }));
    },
    focus() {
      view.focus();
    },
    destroy() {
      view.destroy();
    },
  };
}
