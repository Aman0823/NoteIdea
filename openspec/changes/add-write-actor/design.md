## Context

见 proposal.md 的 Why。当前代码状态与约束：

- `src-tauri/src/lib.rs` 单文件承载全部 Rust 逻辑（约 270 行）：托盘、热键、窗口预热、单实例、延迟测量。`capture` 直接 `OpenOptions::append` 写 `vault/inbox.md`，vault 路径由 `cfg!(debug_assertions)` 分支硬编码。
- 前端三个窗口标签：`main`（测量看板）、`quick`（速记条，启动即预热并隐藏）。便签窗口尚不存在。
- 已验证 NFR-2：热键→可输入 p95 = 27ms，余量充足。
- Tauri 2 的 `win.emit()` 是广播，定向须用 `app.emit_to(label, ...)`；`setup()` 内 emit 的事件早于前端 listen 完成会丢，需配 invoke 命令让前端主动拉取。这两条已在骨架里踩过。
- REQUIREMENTS.md 的 D17 已锁定写入模型，本设计是它的落地方案，不重新论证。

## Goals / Non-Goals

**Goals:**
- 建立 md 写入的唯一通道，使后续四个写者（编辑器、便签、提醒引擎、版本恢复）能直接挂上，无需再改写入层
- 把 `lib.rs` 按职责拆成模块，避免它继续膨胀成一个不可维护的文件
- 保持前端零改动，`invoke('capture', { text })` 签名不变
- 保住 27ms：入队路径不得引入同步文件 IO 或阻塞锁

**Non-Goals:**
- 不实现编辑器、便签、提醒引擎本身，只实现它们要用的写入通道
- 不实现全文搜索索引（persistence spec 里提到索引表，本次只建 schema 不填数据）
- 不实现 CodeMirror 侧的 rebase（`ChangeSet.map()`）——那属于编辑器上线时的工作，本次只保证后端广播的载荷足够前端做 rebase
- 不做 vault 切换 UI，只做首启选择

## Decisions

### D1: actor 用 tokio mpsc + 单 task，而非 Mutex<File> 或线程池

**选择**：`mpsc::UnboundedSender` 投递请求，单个 tokio task 串行消费。

**理由**：单写者语义直接由「只有一个 task 持有文件写权」表达，不依赖调用方自觉加锁。用 `Mutex` 的话，任何一处忘记加锁就破坏保证，而这种 bug 在并发下极难复现。

**代价**：写入变成异步，`capture` 返回时尚未落盘，成功与否靠事件通知。这与 proposal 里标注的内部 BREAKING 一致。

**否决的方案**：`Arc<Mutex<Writer>>` — 编译器不会阻止别人绕过它直接 `fs::write`，保证是纸面上的。

### D2: 入队即持久化，用户已确认保留

写者提交 → 先 INSERT 进 `write_queue` 表 → 再唤醒 actor 处理 → 落盘成功后 DELETE。

**理由**：崩溃恢复要求（file-write spec 的「写队列持久化与崩溃恢复」）。

**代价**：每次写盘多一次 DB 往返，代码多约 80 行。速记条场景下这个开销在毫秒级，且不在热键路径上（热键只负责显示窗口，写入发生在用户按 Enter 之后），不影响 NFR-2。

### D3: 冲突判定用内容哈希，不用 mtime

`base_hash` 存 BLAKE3。

**理由**：Windows 上 mtime 粒度最差 2 秒，且 watcher 事件会把应用自己的写入回流。REQUIREMENTS.md 3.10 已定「不看事件来源，只看内容差异」，这里保持一致。

**否决**：mtime 比对 — 误判率高，且 2 秒窗口内的连续写入无法区分。

### D4: replace_line 双重定位（行号 + 原始内容）

快速路径：哈希匹配则按行号替换。慢速路径：哈希不匹配则全文扫描找 `old_content` 匹配的行。

**这里有个必须现在就定的细节**：若文件中存在多行内容完全相同（例如两行都是 `- [ ] 买菜`），按内容定位会命中第一行，可能改错行。

**决定**：慢速路径下若匹配到多于一行，**拒绝该 ChangeSet 并标为失败**，不猜。理由是改错行是静默的数据损坏，而失败是可见的、用户能处理的。REQUIREMENTS.md 3.6 规定有提醒/贴屏的待办都带 `~id`，那些场景本来就能靠 ID 精确定位；会走到「多行同内容」这条路的只有无 ID 的普通待办，拒绝的代价可接受。

### D5: 模块拆分

```
src-tauri/src/
  lib.rs      仅 run()、Tauri builder 装配、command 注册
  config.rs   Config 读写、vault 路径解析与校验
  vault.rs    vault 目录初始化（inbox.md / .noteidea / assets / .gitignore）
  db.rs       连接管理、schema 迁移、完整性检查、队列表 CRUD
  actor.rs    FileWriteActor、ChangeSet 定义、三种操作的应用逻辑
  window.rs   现有的 show_quick / show_main / 延迟测量（从 lib.rs 挪出）
```

**理由**：lib.rs 已 270 行，本次要加约 500 行。继续堆单文件会让后续每个功能都在同一文件里冲突。

### D6: 首启目录选择器用 Tauri dialog 插件

需新增 `tauri-plugin-dialog` 依赖。未选定 vault 时应用进入「degraded」状态：托盘常驻、主窗口可开并显示选择入口、速记热键触发时提示需先选 vault。

**理由**：用户已明确选择「首启必选」。degraded 状态是这个选择的必然后果——不能默默写到某个用户没同意的目录，也不能因为没选就退出进程（托盘应用退出等于用户以为启动失败）。

### D7: 原子写盘 = 同目录临时文件 + rename

写 `<file>.tmp-<pid>-<seq>` → fsync → `rename` 覆盖目标。

**理由**：同目录保证 rename 在同一卷上是原子的。跨目录（如写到系统 temp）会退化为复制，失去原子性。

**Windows 注意**：`std::fs::rename` 在目标已存在时会失败，须用 `fs::rename` 前先确认或改用平台 API。实现时需实测这一点。

## Risks / Trade-offs

**[异步写入让「保存成功」变得不确定]** → 前端在 emit 的成功事件到达后才认为写入完成；失败时有可见的失败列表（file-write spec 已规定）。速记条的处理是：入队成功即关闭窗口，失败通过托盘通知而非阻塞用户。

**[actor task panic 会导致全应用无法写文件]** → actor 循环内每次处理包 `catch_unwind`，单个 ChangeSet 的 panic 只让该条失败，不杀死 task。task 本身若仍意外退出，`send` 会返回 `Err`，command 层将其作为写入失败上报，用户至少能看到「写不进去了」而不是静默丢数据。

**[队列持久化引入 DB 与文件两个失败点]** → 事务边界明确：DB 事务在文件写成功后才 commit（persistence spec 的「写入事务一致性」）。反向的失败（文件写成功但 DB commit 失败）会导致重启后重放一次该 ChangeSet，因此三种操作都必须幂等或可安全重放——`append` 重放会重复一行，这是唯一不幂等的操作，需在队列记录里存一个 `applied_marker`，重启恢复时先比对文件末尾内容再决定是否重放。

**[Windows 文件锁]** → 外部编辑器（VS Code）打开文件时通常不持排他锁，但杀软扫描可能短暂锁定。这正是重试 3 次要解决的场景。

**[拆模块与现有代码的冲突]** → 本次改动会大幅重排 lib.rs。骨架已验证的行为（托盘、热键、预热、单实例、27ms）必须在拆分后逐项回归，不能只看编译通过。

## Migration Plan

1. 先拆模块但不改行为，编译 + 手动回归（热键、托盘、单实例、延迟）通过后再动写入逻辑。这样如果延迟回归失败，能确定是拆分引入的还是 actor 引入的。
2. 加 config + vault 初始化，此时 `capture` 仍直写文件，只是路径来源变了。
3. 加 DB 与 actor，`capture` 切换到入队。
4. 回归延迟测量，确认 p95 仍在 200ms 内。

**回滚**：每步独立可回退。若 actor 引入无法解决的问题，可暂时让 `capture` 走直写（保留旧路径代码一个提交周期），但这只是应急，不作为长期状态。

## Open Questions

- `write_queue` 表在极端情况下可能堆积（用户长时间无写权限却持续速记）。是否需要队列长度上限、超限如何处理，可在实测中观察后决定，不影响当前 spec 与任务拆分。
