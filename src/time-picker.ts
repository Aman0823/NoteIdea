import { getCurrentWindow } from '@tauri-apps/api/window'
import { emit } from '@tauri-apps/api/event'

const dateList = document.getElementById('date-list')!
const timeInput = document.getElementById('time-input')!
const backBtn = document.getElementById('back-btn')!
const hourInput = document.getElementById('hour-input') as HTMLInputElement
const minuteInput = document.getElementById('minute-input') as HTMLInputElement
const confirmBtn = document.getElementById('confirm-btn')!

let selectedDateType: string = ''

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
  const hour = hourInput.value.padStart(2, '0')
  const minute = minuteInput.value.padStart(2, '0')

  // 计算日期
  const now = new Date()
  let targetDate = new Date(now)

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

  const year = targetDate.getFullYear()
  const month = String(targetDate.getMonth() + 1).padStart(2, '0')
  const day = String(targetDate.getDate()).padStart(2, '0')

  const result = `@${year}-${month}-${day} ${hour}:${minute}`

  // 发送到速记条
  await emit('time-picker:selected', result)

  // 关闭窗口
  await getCurrentWindow().close()
})

// ESC 关闭窗口
document.addEventListener('keydown', (e) => {
  if (e.key === 'Escape') {
    getCurrentWindow().close()
  }
})
