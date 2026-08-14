// 待办行解析结果的 TS 类型，与 Rust 的 todo::syntax 对应
// 不复制任何语法规则——语法由 Rust 唯一定义（D1）

export interface Span {
  start: number
  end: number
}

export type DatePart =
  | { kind: 'absolute'; year: number; month: number; day: number }
  | { kind: 'today' }
  | { kind: 'tomorrow' }

export interface TimeExpr {
  date?: DatePart
  time?: [number, number] // [hour, minute]
}

export type Recurrence =
  | { kind: 'once' }
  | { kind: 'daily' }
  | { kind: 'weekly' }
  | { kind: 'monthly' }
  | { kind: 'yearly' }
  | { kind: 'weekdays' }
  | { kind: 'every_days'; value: number }
  | { kind: 'every_weeks'; value: number }

export type Intensity = { kind: 'toast' } | { kind: 'ring' } | { kind: 'full' }

export type MarkerValue =
  | { kind: 'time'; value: TimeExpr }
  | { kind: 'repeat'; value: Recurrence[] }
  | { kind: 'tag'; value: string }
  | { kind: 'intensity'; value: Intensity }
  | { kind: 'id'; value: string }

export interface Marker {
  value: MarkerValue
  span: Span
}

export interface Degraded {
  suspected: string // "time" | "repeat" | "tag" | "intensity" | "id"
  span: Span
}

export interface TodoLine {
  checked: boolean
  content: Span
  markers: Marker[]
  degraded: Degraded[]
}
