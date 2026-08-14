// 输入辅助弹层：宿主无关组件（D2, design Risks）
// 只接收锚点坐标与过滤词，不直接操作宿主控件

import { invoke } from '@tauri-apps/api/core'

type AssistKind = 'time' | 'repeat' | 'tag' | 'intensity'

interface AssistItem {
  label: string
  value: string // 序列化后的标记文本（含前导字符 @ ! # ^）
}

// 弹层状态
let currentLayer: HTMLElement | null = null
let currentKind: AssistKind | null = null
let selectedIndex = 0
let items: AssistItem[] = []
let onConfirm: ((value: string) => void) | null = null

// 显示弹层
export function show(
  kind: AssistKind,
  anchorX: number,
  anchorY: number,
  filter: string,
  confirm: (value: string) => void
): void {
  hide()

  currentKind = kind
  onConfirm = confirm
  selectedIndex = 0

  // 根据类型生成候选项
  switch (kind) {
    case 'time':
      items = buildTimeItems(filter)
      break
    case 'repeat':
      items = buildRepeatItems(filter)
      break
    case 'tag':
      // 异步加载标签，先显示空弹层
      items = []
      loadTags(filter)
      break
    case 'intensity':
      items = buildIntensityItems(filter)
      break
  }

  // 创建弹层 DOM
  currentLayer = document.createElement('div')
  currentLayer.className = 'assist-layer'
  currentLayer.style.left = `${anchorX}px`
  currentLayer.style.top = `${anchorY + 12}px` // 锚点下方 12px，避免盖住输入内容

  renderItems()
  document.body.appendChild(currentLayer)

  // 撑开窗口（弹层需要额外高度）
  const layerHeight = Math.min(items.length * 32 + 16, 200) // 每项 32px + 上下 padding，最高 200
  invoke('resize_quick', { height: 60 + layerHeight + 8 })

  // 键盘事件
  document.addEventListener('keydown', handleKeyDown)
}

// 隐藏弹层
export function hide(): void {
  if (currentLayer) {
    currentLayer.remove()
    currentLayer = null
    currentKind = null
    onConfirm = null
    items = []
    document.removeEventListener('keydown', handleKeyDown)

    // 恢复窗口高度
    invoke('resize_quick', { height: 60 })
  }
}

// 更新过滤词
export function updateFilter(filter: string): void {
  if (!currentKind) return

  // 重新生成候选项
  switch (currentKind) {
    case 'time':
      items = buildTimeItems(filter)
      break
    case 'repeat':
      items = buildRepeatItems(filter)
      break
    case 'tag':
      loadTags(filter)
      return // 异步更新
    case 'intensity':
      items = buildIntensityItems(filter)
      break
  }

  selectedIndex = Math.min(selectedIndex, Math.max(0, items.length - 1))
  renderItems()
}

// 渲染候选项列表
function renderItems(): void {
  if (!currentLayer) return

  currentLayer.innerHTML = ''

  if (items.length === 0) {
    const empty = document.createElement('div')
    empty.className = 'assist-empty'
    empty.textContent = currentKind === 'tag' ? '加载中...' : '无匹配项'
    currentLayer.appendChild(empty)
    return
  }

  items.forEach((item, i) => {
    const el = document.createElement('div')
    el.className = 'assist-item' + (i === selectedIndex ? ' selected' : '')
    el.textContent = item.label
    el.addEventListener('click', () => confirmItem(i))
    currentLayer!.appendChild(el)
  })
}

// 键盘导航
function handleKeyDown(e: KeyboardEvent): void {
  if (!currentLayer) return

  switch (e.key) {
    case 'ArrowDown':
      e.preventDefault()
      selectedIndex = (selectedIndex + 1) % items.length
      renderItems()
      break
    case 'ArrowUp':
      e.preventDefault()
      selectedIndex = (selectedIndex - 1 + items.length) % items.length
      renderItems()
      break
    case 'Enter':
      e.preventDefault()
      if (items.length > 0) {
        confirmItem(selectedIndex)
      }
      break
    case 'Escape':
      e.preventDefault()
      hide()
      break
  }
}

// 确认选中项
function confirmItem(index: number): void {
  if (!onConfirm || index < 0 || index >= items.length) return
  const value = items[index].value
  const callback = onConfirm  // 保存回调
  hide()
  callback(value)  // hide 之后调用
}

// 构建时间候选项
function buildTimeItems(filter: string): AssistItem[] {
  const now = new Date()
  const result: AssistItem[] = []

  // 今天的几个时间点
  const todayTimes = ['09:00', '12:00', '14:00', '18:00', '20:00', '22:00']
  for (const time of todayTimes) {
    const value = `@${formatDate(now)} ${time}`
    if (matches(value, filter)) {
      result.push({ label: `今天 ${time}`, value })
    }
  }

  // 明天的几个时间点
  const tomorrow = new Date(now)
  tomorrow.setDate(tomorrow.getDate() + 1)
  const tomorrowTimes = ['09:00', '12:00', '18:00', '20:00']
  for (const time of tomorrowTimes) {
    const value = `@${formatDate(tomorrow)} ${time}`
    if (matches(value, filter)) {
      result.push({ label: `明天 ${time}`, value })
    }
  }

  // 后天 09:00
  const dayAfter = new Date(now)
  dayAfter.setDate(dayAfter.getDate() + 2)
  const dayAfter0900 = `@${formatDate(dayAfter)} 09:00`
  if (matches(dayAfter0900, filter)) {
    result.push({ label: '后天 09:00', value: dayAfter0900 })
  }

  // 本周五 18:00（如果今天不是周五或之后）
  const friday = getNextWeekday(now, 5)
  if (friday) {
    const friday1800 = `@${formatDate(friday)} 18:00`
    if (matches(friday1800, filter)) {
      result.push({ label: '本周五 18:00', value: friday1800 })
    }
  }

  // 下周一 09:00
  const nextMonday = getNextWeekday(now, 1, true)
  const nextMonday0900 = `@${formatDate(nextMonday)} 09:00`
  if (matches(nextMonday0900, filter)) {
    result.push({ label: '下周一 09:00', value: nextMonday0900 })
  }

  // 下周五 18:00
  const nextFriday = getNextWeekday(now, 5, true)
  const nextFriday1800 = `@${formatDate(nextFriday)} 18:00`
  if (matches(nextFriday1800, filter)) {
    result.push({ label: '下周五 18:00', value: nextFriday1800 })
  }

  return result
}

// 构建重复规则候选项
function buildRepeatItems(filter: string): AssistItem[] {
  const all: AssistItem[] = [
    { label: '每天', value: '!daily' },
    { label: '工作日', value: '!weekdays' },
    { label: '每周', value: '!weekly' },
    { label: '每月', value: '!monthly' },
    { label: '每年', value: '!yearly' },
  ]
  return all.filter(item => matches(item.value, filter))
}

// 构建强度候选项
function buildIntensityItems(filter: string): AssistItem[] {
  const all: AssistItem[] = [
    { label: '轻提示', value: '^toast' },
    { label: '响铃', value: '^ring' },
    { label: '全屏强提醒', value: '^full' },
  ]
  return all.filter(item => matches(item.value, filter))
}

// 异步加载标签
async function loadTags(filter: string): Promise<void> {
  try {
    const tags: [string, number][] = await invoke('list_tags')
    items = tags
      .filter(([tag]) => matches(`#${tag}`, filter))
      .map(([tag]) => ({ label: tag, value: `#${tag}` }))

    // 支持新建标签：如果 filter 非空且不在已有列表中
    if (filter && !tags.some(([tag]) => tag === filter)) {
      items.unshift({ label: `新建：${filter}`, value: `#${filter}` })
    }

    selectedIndex = 0
    renderItems()
  } catch (e) {
    console.error('加载标签失败:', e)
    items = []
    renderItems()
  }
}

// 简单匹配：value 包含 filter（不区分大小写）
function matches(value: string, filter: string): boolean {
  if (!filter) return true
  return value.toLowerCase().includes(filter.toLowerCase())
}

// 格式化日期为 YYYY-MM-DD
function formatDate(d: Date): string {
  const y = d.getFullYear()
  const m = String(d.getMonth() + 1).padStart(2, '0')
  const day = String(d.getDate()).padStart(2, '0')
  return `${y}-${m}-${day}`
}

// 获取下一个指定星期几（0=周日, 1=周一, ...）
function getNextWeekday(from: Date, targetDay: number, forceNext = false): Date {
  const result = new Date(from)
  const currentDay = result.getDay()
  let daysToAdd = targetDay - currentDay

  if (daysToAdd <= 0 || forceNext) {
    daysToAdd += 7
  }

  result.setDate(result.getDate() + daysToAdd)
  return result
}
