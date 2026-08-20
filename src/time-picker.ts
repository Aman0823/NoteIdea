import { invoke } from '@tauri-apps/api/core'
import { emit } from '@tauri-apps/api/event'
import type { MarkerValue } from './types/todo'

// 关窗必须走 Rust：capability 只给了 core:default，前端直接
// getCurrentWindow().close() 会被静默拦截，点了跟没点一样。
function closeWindow() {
  void invoke('close_time_picker')
}

// 发起方靠 query 传进来。结果是广播，不带 req 的话主编辑器点 chip
// 会把速记条一起改掉。
const params = new URLSearchParams(location.search)
const kind = params.get('kind') ?? 'time'
const requestId = params.get('req') ?? ''

/** 选中结果一律是**结构化值**，规范文本由 Rust 序列化产出（design E5）。 */
async function confirmValue(value: MarkerValue) {
  await emit('marker-picker:selected', { requestId, value })
  closeWindow()
}

const dateList = document.getElementById('date-list')!
const optionList = document.getElementById('option-list')!
const tagPanel = document.getElementById('tag-panel')!
const timeInput = document.getElementById('time-input')!
const backBtn = document.getElementById('back-btn')!
const hourInput = document.getElementById('hour-input') as HTMLInputElement
const minuteInput = document.getElementById('minute-input') as HTMLInputElement
const confirmBtn = document.getElementById('confirm-btn')!

let selectedDateType: string = ''

// ---------- 按 kind 决定显示哪个面板 ----------

interface Option {
  label: string
  value: MarkerValue
}

const REPEAT_OPTIONS: Option[] = [
  { label: '不重复', value: { kind: 'repeat', value: { kind: 'once' } } },
  { label: '每天', value: { kind: 'repeat', value: { kind: 'daily' } } },
  { label: '工作日', value: { kind: 'repeat', value: { kind: 'weekdays' } } },
  { label: '每周', value: { kind: 'repeat', value: { kind: 'weekly' } } },
  { label: '每月', value: { kind: 'repeat', value: { kind: 'monthly' } } },
  { label: '每年', value: { kind: 'repeat', value: { kind: 'yearly' } } },
  { label: '每 3 天', value: { kind: 'repeat', value: { kind: 'every_days', n: 3 } } },
  { label: '每 2 周', value: { kind: 'repeat', value: { kind: 'every_weeks', n: 2 } } },
]

const INTENSITY_OPTIONS: Option[] = [
  { label: '轻提示', value: { kind: 'intensity', value: 'toast' } },
  { label: '响铃', value: { kind: 'intensity', value: 'ring' } },
  { label: '全屏强提醒', value: { kind: 'intensity', value: 'full' } },
]

function renderOptions(options: Option[]) {
  dateList.classList.add('hidden')
  optionList.classList.remove('hidden')
  optionList.replaceChildren()
  for (const opt of options) {
    const el = document.createElement('div')
    el.className = 'option-item'
    el.textContent = opt.label
    el.addEventListener('click', () => void confirmValue(opt.value))
    optionList.append(el)
  }
}

switch (kind) {
  case 'repeat':
    renderOptions(REPEAT_OPTIONS)
    break
  case 'intensity':
    renderOptions(INTENSITY_OPTIONS)
    break
  case 'tag': {
    dateList.classList.add('hidden')
    tagPanel.classList.remove('hidden')
    const tagInput = document.getElementById('tag-input') as HTMLInputElement
    const submitTag = () => {
      const name = tagInput.value.trim()
      if (name === '') return
      void confirmValue({ kind: 'tag', value: name })
    }
    document.getElementById('tag-confirm')!.addEventListener('click', submitTag)
    tagInput.addEventListener('keydown', (e) => {
      if (e.key === 'Enter') submitTag()
    })
    setTimeout(() => tagInput.focus(), 0)
    break
  }
  default:
    // time：保持原有的两级流程
    break
}

// ---------- 时间选择（kind = time） ----------

// 日期类型点击
dateList.addEventListener('click', (e) => {
  const item = (e.target as HTMLElement).closest('.date-item')
  if (!item) return

  selectedDateType = item.getAttribute('data-type')!

  // 切换到时间输入界面
  dateList.classList.add('hidden')
  timeInput.classList.remove('hidden')

  // 根据日期类型设置默认时间
  const now = new Date()
  const currentHour = now.getHours()

  if (selectedDateType === 'today') {
    // 今天：默认当前小时 +1，分钟 00
    const nextHour = (currentHour + 1) % 24
    hourInput.value = String(nextHour).padStart(2, '0')
  } else {
    // 其他日期：默认 09:00
    hourInput.value = '09'
  }
  minuteInput.value = '00'

  hourInput.focus()
  hourInput.select()
})

// 返回按钮
backBtn.addEventListener('click', () => {
  timeInput.classList.add('hidden')
  dateList.classList.remove('hidden')
})

// 微调按钮
document.querySelectorAll('.spin-btn').forEach((btn) => {
  btn.addEventListener('click', () => {
    const target = btn.getAttribute('data-target')!
    const dir = parseInt(btn.getAttribute('data-dir')!)

    const input = target === 'hour' ? hourInput : minuteInput
    const max = target === 'hour' ? 23 : 59

    let val = parseInt(input.value) || 0
    val = (val + dir + max + 1) % (max + 1)
    input.value = String(val).padStart(2, '0')
  })
})

// 输入框：只允许数字，自动补零
function setupInput(input: HTMLInputElement, max: number) {
  input.addEventListener('input', () => {
    input.value = input.value.replace(/\D/g, '')
    if (input.value.length > 2) {
      input.value = input.value.slice(0, 2)
    }
  })

  input.addEventListener('blur', () => {
    let val = parseInt(input.value) || 0
    if (val > max) val = max
    input.value = String(val).padStart(2, '0')
  })

  input.addEventListener('keydown', (e) => {
    if (e.key === 'Enter') {
      confirmBtn.click()
    }
  })
}

setupInput(hourInput, 23)
setupInput(minuteInput, 59)

// 确定按钮
confirmBtn.addEventListener('click', async () => {
  const hour = parseInt(hourInput.value) || 0
  const minute = parseInt(minuteInput.value) || 0

  // 计算日期
  const now = new Date()
  const targetDate = new Date(now)

  switch (selectedDateType) {
    case 'today':
      // 今天
      break
    case 'tomorrow':
      targetDate.setDate(targetDate.getDate() + 1)
      break
    case 'dayAfter':
      targetDate.setDate(targetDate.getDate() + 2)
      break
    case 'thisFri':
      // 本周五
      {
        const day = targetDate.getDay()
        const daysToFri = day <= 5 ? 5 - day : 5 + (7 - day)
        targetDate.setDate(targetDate.getDate() + daysToFri)
      }
      break
    case 'nextMon':
      // 下周一
      {
        const day = targetDate.getDay()
        const daysToNextMon = day === 0 ? 1 : 8 - day
        targetDate.setDate(targetDate.getDate() + daysToNextMon)
      }
      break
    case 'nextFri':
      // 下周五
      {
        const day = targetDate.getDay()
        const daysToNextFri = day <= 5 ? 5 - day + 7 : 12 - day
        targetDate.setDate(targetDate.getDate() + daysToNextFri)
      }
      break
    case 'custom':
      // 自定义日期（暂时留空，后续实现日历选择器）
      alert('自定义日期功能尚未实现')
      return
  }

  // 只送结构，不拼文本：`@2026-08-14 18:00` 这串字由 Rust 序列化产出
  await confirmValue({
    kind: 'time',
    value: {
      date: {
        kind: 'absolute',
        year: targetDate.getFullYear(),
        month: targetDate.getMonth() + 1,
        day: targetDate.getDate(),
      },
      time: [hour, minute],
    },
  })
})

// ESC 关闭窗口
document.addEventListener('keydown', (e) => {
  if (e.key === 'Escape') {
    closeWindow()
  }
})
