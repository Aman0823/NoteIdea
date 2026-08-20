## Why

主窗口至今是一块延迟看板。第一期的三件基础工程里，3.9 写入通道（`add-write-actor`）和 3.3/3.6 的语法与身份层（`add-todo-syntax-and-identity`）都已立项，但**用户还没有一处可以真正写笔记的地方**——FR-3 的 CM6 编辑器是这条链路的出口，没有它，前面所有地基都没有消费方。

这个 change 把 FR-2（文件树）+ FR-3（所见即所得编辑器）+ FR-4（自动保存）接起来，让「打开一篇笔记 → 敲字 → 敲 `@` 设提醒 → 关窗再开还在」这条闭环第一次跑通。

同时它暴露了一个必须现在解决的架构缺口：**actor 现有四种 op（`append` / `replace_line` / `create` / `replace_file`）都承载不了编辑器的任意输入**。3.9 写明主编辑器按字符偏移提交，但那种 op 还不存在。编辑器不能用 `replace_file`（直接违反 D17 规则二），所以 op 必须这一期加。

### 开工前的代码核对结论

`add-todo-syntax-and-identity` 的 tasks.md 此前被整体打勾，实际有四处是空的（已在该文件里逐项撤回打勾）：

| 声称完成 | 实际 |
|---|---|
| 组 3 序列化 | `write_marker_to_line` **不存在**。`identity.rs` 里调用了它，因为该文件没编译所以没报错 |
| 组 7 身份层 | `identity.rs` 写了 359 行含 8 个测试，但 `todo/mod.rs` 只声明了 `syntax` 和 `index`，**从未参与编译**；`allocate_todo_id` 返回 `Err("尚未实现")`；`invoke_handler` 里没有 `replace_line` / `create` |
| 组 8 索引 | `index.rs` 共 25 行，函数体是 `// TODO: 实际扫描逻辑`；`list_tags` 返回 `Err("尚未实现")` |
| 组 2 时间求值 | 只有 `parse_time_expr`（文本 → 结构），没有任何把结构变成绝对时间的函数 |

`cargo test` 实测 43 个通过，其中无一条测序列化或时间求值。

这一期只补编辑器**真正依赖**的那部分（组 3 序列化、`identity.rs` 接进编译、`replace_line` / `create` 暴露成 command）。组 2 时间求值与组 8 索引不是编辑器的依赖，留给提醒引擎和聚合视图那一期。

## What Changes

- `vault/file-write` 新增 `apply_edits` op：携带一组 `{from, to, insert}` 字符偏移变更与严格基线哈希。基线不匹配一律拒绝，由前端 rebase 后重投（3.9 原文的做法）
- 新增 CM6 编辑器：markdown 高亮 + Typora 式所见即所得（光标所在行显示原始语法，离开即渲染）+ 3.5 的五种标记 chip（`~id` 完全隐藏）+ GFM 复选框可点击
- `src/assist.ts` 接进 CM6：编辑器里敲 `@` `!` `#` `^` 弹与速记条同一套选择层，插入的文本一律由 Rust 序列化产出
- 新增自动保存：停止输入 800ms 落盘；窗口失焦、切换笔记、退出立即落盘。全部经 actor
- 新增文件树：列出 vault 内 md，可打开、可新建。重命名 / 删除 / 拖动移动不在这一期
- 补齐 `todo/syntax` 已立规但未实现的序列化方向（`write_marker_to_line`）——chip 点开改时间靠它回写
- 把 `identity.rs` 接进 `todo/mod.rs` 并让它的测试真的跑绿；`allocate_todo_id` 实装

## Non-goals

- **表格编辑与图片内联显示**：FR-3 里列了，但它们是纯增量的 decoration 工作，不影响架构。先把 chip 与所见即所得的 decoration 骨架定对
- ~~**代码块内的语言高亮**：只做 markdown 自身高亮，不装各语言 lang 包（体积与收益不成比例）~~
  **已改为纳入范围（实现组 5 时用户要求）。** 走 `@codemirror/language-data` 的按需加载：
  143 种语言（含只存在于 legacy-modes 的 `http` / `nginx` 等）注册为 `LanguageDescription`，
  用到哪种才动态 import 哪种。原先「体积与收益不成比例」的判断建立在「静态打进主包」
  的假设上，那个假设不成立——实测主 chunk 只从 531 kB 涨到 550 kB，速记条 chunk
  完全不受影响（NFR-2 的路径没被碰到），代价只是 dist 多出百来个惰性小 chunk。
- **文件监听与外部编辑冲突弹窗（FR-6）**：本期编辑器是唯一持缓冲的写者，冲突面极小。watcher 与二选一弹窗一起做
- **文件树的重命名 / 删除 / 拖动移动（FR-2 剩余部分）**：这些是文件系统操作，与编辑器架构无关
- **组 2 时间求值、组 8 待办索引、`list_tags`**：编辑器不依赖。`#` 弹层这一期只能新建标签，列不出已有标签
- **聚合视图、便签、提醒引擎**：都在编辑器之后

## Capabilities

### New Capabilities

- `editor/buffer-sync`: 编辑缓冲与磁盘的同步协议。规定编辑器提交什么、何时提交、基线冲突时如何 rebase 重投、以及撤销栈的边界（D20）
- `editor/wysiwyg`: 所见即所得渲染。规定原始语法何时显示何时被 chip 取代、chip 的交互、解析结果异步到达期间的渲染行为、以及渲染层绝不改写文本这条底线
- `notes/file-tree`: 笔记浏览。规定文件树的数据来源、刷新时机、以及切换笔记时未落盘内容的处理

### Modified Capabilities

- `vault/file-write`: 新增 `apply_edits` op 与它的严格基线语义（现有「基线已变则按内容重定位」只适用于 `replace_line`，字符偏移无法重定位）

## Impact

**代码**
- 新增 `src/editor/`：CM6 装配、decoration 插件、assist 适配层、缓冲同步
- `src/main.ts` + `index.html`：延迟看板降级为一个折叠面板，主体换成文件树 + 编辑器
- `src-tauri/src/actor.rs`：新增 `Operation::ApplyEdits` 与其应用逻辑、测试
- `src-tauri/src/todo/syntax.rs`：补序列化（`write_marker_to_line`、标记序列化）
- `src-tauri/src/todo/mod.rs`：声明 `identity`
- `src-tauri/src/lib.rs`：新增 `list_notes` / `read_note` / `replace_line` / `create` / `apply_edits` / `parse_todo_lines`（批量）command；`allocate_todo_id` 实装

**依赖**
- 新增 CodeMirror 6：`@codemirror/state` `@codemirror/view` `@codemirror/commands` `@codemirror/language` `@codemirror/lang-markdown`，全部锁定确定版本
- CM6 只进主窗口入口，**不得进 `quick.ts`**——速记条的首帧可输入时间是 NFR-2 的成败线，不能被编辑器体积拖累

**数据**
- vault 内 md 开始被用户的真实输入改写。这是 actor 第一次承接非行级的变更

**风险**
- CM6 的 decoration 架构一旦定错，chip、输入辅助、复选框都要重写。这是这一期唯一需要慎重的地方，design 里逐条定
