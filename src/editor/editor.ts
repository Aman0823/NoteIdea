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
import { languages } from '@codemirror/language-data';
import { syntaxHighlighting, HighlightStyle, indentOnInput } from '@codemirror/language';
import { tags } from '@lezer/highlight';
import { todoChips, markerInteraction, flushRequest } from './decorations';
import { assistInput } from './assist-adapter';

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
  // ---- markdown 自身 ----
  { tag: tags.heading, color: '#f5c542', fontWeight: '600' },
  { tag: tags.emphasis, fontStyle: 'italic' },
  { tag: tags.strong, fontWeight: '700' },
  { tag: tags.monospace, color: '#4ade80' },
  { tag: tags.link, color: '#7aa2f7' },
  { tag: tags.url, color: '#7aa2f7' },
  { tag: tags.quote, color: '#9a94ad', fontStyle: 'italic' },
  { tag: tags.processingInstruction, color: '#9a94ad' },
  { tag: tags.meta, color: '#9a94ad' },

  // ---- 围栏代码块内的嵌套语言 ----
  // markdown 本身不产出这些 tag，所以不会和上面打架。
  { tag: tags.comment, color: '#6f6885', fontStyle: 'italic' },
  { tag: [tags.keyword, tags.modifier, tags.controlKeyword], color: '#bb9af7' },
  { tag: [tags.string, tags.special(tags.string)], color: '#9ece6a' },
  { tag: [tags.number, tags.bool, tags.null, tags.atom], color: '#ff9e64' },
  { tag: [tags.escape, tags.regexp], color: '#b4f9f8' },
  { tag: [tags.typeName, tags.className, tags.namespace], color: '#2ac3de' },
  { tag: [tags.function(tags.variableName), tags.function(tags.propertyName)], color: '#7aa2f7' },
  { tag: tags.propertyName, color: '#7dcfff' },
  { tag: tags.definition(tags.variableName), color: '#e8e6f0' },
  { tag: tags.variableName, color: '#c8c3d8' },
  { tag: [tags.operator, tags.operatorKeyword], color: '#89ddff' },
  { tag: [tags.punctuation, tags.separator, tags.bracket], color: '#8a84a0' },
  { tag: tags.tagName, color: '#f7768e' },
  { tag: tags.attributeName, color: '#bb9af7' },
  { tag: tags.attributeValue, color: '#9ece6a' },
  { tag: tags.invalid, color: '#f87171' },
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
    // codeLanguages：围栏代码块按 ```lang 挂对应语言的解析器，语言包按需
    // 动态加载（143 种，含只存在于 legacy-modes 的 http / nginx 之类），
    // 所以主 chunk 不会因此变大。
    markdown({ base: markdownLanguage, codeLanguages: languages }),
    indentOnInput(),
    syntaxHighlighting(mdHighlight),
    todoChips,
    markerInteraction,
    assistInput,
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
