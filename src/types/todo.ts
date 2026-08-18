// 待办行解析结果的 TS 类型，与 Rust 的 todo::syntax 对应
// 不复制任何语法规则——语法由 Rust 唯一定义（D1）
//
// 形状由 Rust 侧的 test_json_shape_is_what_frontend_expects 钉死。
// 改这里之前先看那个测试，别凭印象改。

export interface Span {
  /** UTF-8 字节偏移，闭区间 [start, end] */
  start: number
  end: number
}

export type DatePart =
  | { kind: 'absolute'; year: number; month: number; day: number }
  | { kind: 'today' }
  | { kind: 'tomorrow' }

export interface TimeExpr {
  date: DatePart | null
  time: [number, number] | null // [hour, minute]
}

// 内部带标签：every_Nd / every_Nw 的数字在 `n` 上，不是 `value`
export type Recurrence =
  | { kind: 'once' }
  | { kind: 'daily' }
  | { kind: 'weekly' }
  | { kind: 'monthly' }
  | { kind: 'yearly' }
  | { kind: 'weekdays' }
  | { kind: 'every_days'; n: number }
  | { kind: 'every_weeks'; n: number }

// 裸字符串，不是对象
export type Intensity = 'toast' | 'ring' | 'full'

export type MarkerKind = 'time' | 'repeat' | 'tag' | 'intensity' | 'id'

export type MarkerValue =
  | { kind: 'time'; value: TimeExpr }
  | { kind: 'repeat'; value: Recurrence }
  | { kind: 'tag'; value: string }
  | { kind: 'intensity'; value: Intensity }
  | { kind: 'id'; value: string }

export interface Marker {
  value: MarkerValue
  span: Span
}

export interface Degraded {
  suspected: MarkerKind
  span: Span
}

export interface TodoLine {
  checked: boolean
  content: Span
  markers: Marker[]
  degraded: Degraded[]
  /** 引号屏蔽区间（含引号本身） */
  quoted: Span[]
}
