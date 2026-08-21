// 输入辅助弹层：宿主无关组件（D2, design Risks）
//
// 宿主可以是速记条的输入框，也可以是主编辑器的 CM6 实例。这里只负责
// 「显示候选、键盘导航、把选中的**结构化值**交回去」，不碰任何宿主控件、
// 不假设自己在哪个窗口里。
//
// 两条硬边界：
//   1. **不产出文本**。候选项携带的是 MarkerValue 结构，规范文本一律由
//      Rust 的 write_marker 产出——前端拼出来的格式一旦与解析器有出入，
//      就是写进文件、再也读不回来的脏数据（design E5）。
//   2. **不动窗口**。旧版在这里直接 invoke('resize_quick') 撑高速记条，
//      搬到编辑器就会去改另一个窗口的高度。宿主自己决定要不要腾地方。

import { invoke } from '@tauri-apps/api/core'
import type { MarkerValue } from './types/todo'

export type AssistKind = 'time' | 'repeat' | 'tag' | 'intensity'

export interface AssistItem {
  label: string
  /** 附注，显示在右侧（如「新建」「尚无索引」）。 */
  hint?: string
  value: MarkerValue
}

/** 宿主需要提供的东西。弹层只通过它与外界打交道。 */
export interface AssistHost {
  /** 弹层锚点的屏幕坐标（通常是光标位置）。 */
  anchor: { x: number; y: number }
  /** 用户选定了某个候选值。 */
  onPick: (value: MarkerValue) => void
  /**
   * 弹层被**取消**（Esc、失焦、触发字符被删掉）。
   *
   * 选中确认走 `onPick`，不会再调这里——两者互斥。曾经二者都调，且 onClose
   * 先于 onPick，宿主在 onClose 里清掉的状态导致 onPick 做不成任何事。
   */
  onClose?: () => void
  /** 弹层撑开/收起时通知宿主，速记条用它调窗口高度；编辑器不需要。 */
  onHeightChange?: (height: number) => void
}

let layer: HTMLElement | null = null
let currentKind: AssistKind | null = null
let currentHost: AssistHost | null = null
let selectedIndex = 0
let items: AssistItem[] = []
/** 正在走「选中确认」路径：此时关层不应触发 onClose（二者互斥）。 */
let picking = false

export function isOpen(): boolean {
  return layer !== null
}

export function show(kind: AssistKind, filter: string, host: AssistHost): void {
  hide()

  currentKind = kind
  currentHost = host
  selectedIndex = 0
  items = buildItems(kind, filter)

  layer = document.createElement('div')
  layer.className = 'assist-layer'
  layer.style.left = `${host.anchor.x}px`
  layer.style.top = `${host.anchor.y + 12}px` // 锚点下方 12px，避免盖住正在输入的内容

  renderItems()
  document.body.appendChild(layer)

  // 标签要异步取，先把层显示出来再补
  if (kind === 'tag') void loadTags(filter)

  document.addEventListener('keydown', handleKeyDown, true)
}

export function hide(): void {
  if (layer === null) return

  layer.remove()
  layer = null
  currentKind = null
  items = []
  document.removeEventListener('keydown', handleKeyDown, true)

  const host = currentHost
  currentHost = null
  host?.onHeightChange?.(0)
  if (!picking) host?.onClose?.()
}

/** 触发字符后面又敲了字：用新的过滤词重算候选。 */
export function updateFilter(filter: string): void {
  if (currentKind === null) return

  if (currentKind === 'tag') {
    void loadTags(filter)
    return
  }

  items = buildItems(currentKind, filter)
  selectedIndex = Math.min(selectedIndex, Math.max(0, items.length - 1))
  renderItems()
}

function buildItems(kind: AssistKind, filter: string): AssistItem[] {
  switch (kind) {
    case 'time':
      return buildTimeItems(filter)
    case 'repeat':
      return buildRepeatItems(filter)
    case 'intensity':
      return buildIntensityItems(filter)
    case 'tag':
      return [] // 异步补
  }
}

function renderItems(): void {
  if (layer === null) return

  layer.replaceChildren()

  if (items.length === 0) {
    const empty = document.createElement('div')
    empty.className = 'assist-empty'
    empty.textContent = currentKind === 'tag' ? '加载中…' : '无匹配项'
    layer.append(empty)
  } else {
    items.forEach((item, i) => {
      const el = document.createElement('div')
      el.className = 'assist-item' + (i === selectedIndex ? ' selected' : '')

      const label = document.createElement('span')
      label.textContent = item.label
      el.append(label)

      if (item.hint !== undefined) {
        const hint = document.createElement('span')
        hint.className = 'assist-hint'
        hint.textContent = item.hint
        el.append(hint)
      }

      // 用 mousedown 而非 click：click 之前编辑器可能已经因为失焦收了弹层
      el.addEventListener('mousedown', (e) => {
        e.preventDefault()
        confirmItem(i)
      })
      layer!.append(el)
    })
  }

  currentHost?.onHeightChange?.(Math.min(items.length * 32 + 16, 200))
}

function handleKeyDown(e: KeyboardEvent): void {
  if (layer === null) return

  switch (e.key) {
    case 'ArrowDown':
      if (items.length === 0) return
      e.preventDefault()
      e.stopPropagation()
      selectedIndex = (selectedIndex + 1) % items.length
      renderItems()
      break
    case 'ArrowUp':
      if (items.length === 0) return
      e.preventDefault()
      e.stopPropagation()
      selectedIndex = (selectedIndex - 1 + items.length) % items.length
      renderItems()
      break
    case 'Enter':
      if (items.length === 0 || e.isComposing) return
      e.preventDefault()
      e.stopPropagation()
      confirmItem(selectedIndex)
      break
    case 'Escape':
      // 只收弹层，不让宿主看到这次 Esc（7.3）
      e.preventDefault()
      e.stopPropagation()
      hide()
      break
    default:
      break
  }
}

function confirmItem(index: number): void {
  if (index < 0 || index >= items.length) return
  const value = items[index].value
  const host = currentHost

  picking = true
  hide()
  picking = false

  host?.onPick(value)
}

// ---------- 候选项 ----------
//
// 这些函数只产出**结构**与**显示用文案**，绝不产出要写进文档的文本。

function matches(label: string, filter: string): boolean {
  if (filter === '') return true
  return label.toLowerCase().includes(filter.toLowerCase())
}

function pad2(n: number): string {
  return String(n).padStart(2, '0')
}

function dateValue(d: Date): MarkerValue {
  return {
    kind: 'time',
    value: {
      date: { kind: 'absolute', year: d.getFullYear(), month: d.getMonth() + 1, day: d.getDate() },
      time: null,
    },
  }
}

function at(d: Date, hour: number, minute: number): MarkerValue {
  const v = dateValue(d)
  if (v.kind === 'time') v.value.time = [hour, minute]
  return v
}

function shiftDays(from: Date, days: number): Date {
  const d = new Date(from)
  d.setDate(d.getDate() + days)
  return d
}

/** 下一个指定星期几（0=周日）。`forceNext` 为真时即便今天就是也跳到下周。 */
function nextWeekday(from: Date, target: number, forceNext = false): Date {
  const d = new Date(from)
  let delta = target - d.getDay()
  if (delta <= 0 || forceNext) delta += 7
  d.setDate(d.getDate() + delta)
  return d
}

function buildTimeItems(filter: string): AssistItem[] {
  const now = new Date()
  const out: AssistItem[] = []

  const push = (label: string, d: Date, h: number, m: number) => {
    if (matches(label, filter)) out.push({ label, hint: `${pad2(h)}:${pad2(m)}`, value: at(d, h, m) })
  }

  for (const [h, m] of [[9, 0], [12, 0], [14, 0], [18, 0], [20, 0], [22, 0]] as const) {
    push('今天', now, h, m)
  }
  for (const [h, m] of [[9, 0], [12, 0], [18, 0], [20, 0]] as const) {
    push('明天', shiftDays(now, 1), h, m)
  }
  push('后天', shiftDays(now, 2), 9, 0)
  push('本周五', nextWeekday(now, 5), 18, 0)
  push('下周一', nextWeekday(now, 1, true), 9, 0)
  push('下周五', nextWeekday(now, 5, true), 18, 0)

  return out
}

function buildRepeatItems(filter: string): AssistItem[] {
  const all: AssistItem[] = [
    { label: '每天', value: { kind: 'repeat', value: { kind: 'daily' } } },
    { label: '工作日', value: { kind: 'repeat', value: { kind: 'weekdays' } } },
    { label: '每周', value: { kind: 'repeat', value: { kind: 'weekly' } } },
    { label: '每月', value: { kind: 'repeat', value: { kind: 'monthly' } } },
    { label: '每年', value: { kind: 'repeat', value: { kind: 'yearly' } } },
    { label: '每 3 天', value: { kind: 'repeat', value: { kind: 'every_days', n: 3 } } },
    { label: '每 2 周', value: { kind: 'repeat', value: { kind: 'every_weeks', n: 2 } } },
    { label: '不重复', hint: '取消', value: { kind: 'repeat', value: { kind: 'once' } } },
  ]
  return all.filter((i) => matches(i.label, filter))
}

function buildIntensityItems(filter: string): AssistItem[] {
  const all: AssistItem[] = [
    { label: '轻提示', hint: '默认', value: { kind: 'intensity', value: 'toast' } },
    { label: '响铃', value: { kind: 'intensity', value: 'ring' } },
    { label: '全屏强提醒', value: { kind: 'intensity', value: 'full' } },
  ]
  return all.filter((i) => matches(i.label, filter))
}

/**
 * 标签候选。
 *
 * `list_tags` 依赖尚未实现的索引，现在一定失败。**必须如实告诉用户「暂时列
 * 不出已有标签」**，不能显示一个空列表假装这个 vault 里没有标签（7.5）。
 */
async function loadTags(filter: string): Promise<void> {
  const typed = filter.trim()
  const fresh: AssistItem[] = []

  let indexed: [string, number][] | null = null
  try {
    indexed = await invoke<[string, number][]>('list_tags')
  } catch {
    indexed = null
  }

  if (indexed !== null) {
    for (const [tag] of indexed) {
      if (matches(tag, typed)) fresh.push({ label: tag, value: { kind: 'tag', value: tag } })
    }
  }

  if (typed !== '' && !fresh.some((i) => i.label === typed)) {
    fresh.unshift({ label: typed, hint: '新建', value: { kind: 'tag', value: typed } })
  }

  items = fresh
  selectedIndex = 0
  renderItems()

  if (indexed === null && layer !== null) {
    const note = document.createElement('div')
    note.className = 'assist-empty'
    note.textContent =
      typed === '' ? '标签索引还没做，直接输入即可新建' : '标签索引还没做，列不出已有标签'
    layer.append(note)
  }
}
