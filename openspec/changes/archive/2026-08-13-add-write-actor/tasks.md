## 1. 模块拆分（不改行为）

- [x] 1.1 新建 `window.rs`，把 `show_quick`、`show_main`、`Timing`、`mark_ready`、`timings`、`hide_quick`、`quick_warmed` 从 `lib.rs` 挪过去
- [x] 1.2 `lib.rs` 只留 `run()`、Tauri builder 装配、command 注册、托盘与热键的 setup 调用
- [x] 1.3 `cargo build` 通过（clippy 零警告）
- [x] 1.4 手动回归：Alt+Space 弹速记条、Alt+N 弹主窗口、托盘左键与右键菜单、关主窗口进程留存、第二实例聚焦已有实例
- [x] 1.5 回归延迟测量，反复触发 15 次以上，确认 p95 仍在 200ms 内（拆分前基线 27ms）

## 2. Config 与 vault 初始化

- [x] 2.1 加 `tauri-plugin-dialog` 依赖并注册插件
- [x] 2.2 新建 `config.rs`：Config 结构（vault_path、version）、用户级配置目录读写、损坏时重命名保留并用默认值继续
- [x] 2.3 新建 `vault.rs`：初始化 `inbox.md`、`.noteidea/`、`assets/`，向 vault 的 `.gitignore` 追加 `.noteidea/`（已存在则不重复写）
- [x] 2.4 实现 vault 路径校验：存在性、可写性，路径失效时不静默重建而是要求重选
- [x] 2.5 实现首启目录选择器；用户取消时进入 degraded 状态（托盘常驻、功能不可用但有明确提示与重选入口）
- [x] 2.6 degraded 状态下按速记热键，提示需先选 vault，不丢弃用户已输入内容
- [x] 2.7 `capture` 改为从 Config 取 vault 路径（此步仍是直写文件，只换路径来源）
- [x] 2.8 验证：空目录选为 vault 后结构补齐；已有 vault 重开不覆盖任何文件；删掉 inbox.md 重启后自动重建

## 3. SQLite 持久化层

- [x] 3.1 加 `rusqlite`（bundled）依赖
- [x] 3.2 新建 `db.rs`：连接打开、WAL 模式、`PRAGMA integrity_check`
- [x] 3.3 建 schema：`write_queue`、`reminders`、`occurrences`、`stickies`、`todos`、`app_state` + 索引 + 版本记录
- [x] 3.4 实现迁移框架（读 schema 版本，按需升级；失败按损坏处理而非 panic）
- [x] 3.5 实现损坏与版本不匹配的处理：重命名旧库、建新库、不阻塞启动
- [x] 3.6 实现 DB 文件独占：`BEGIN IMMEDIATE` 探测写锁；**被占用时绝不重建**（会毁掉对方的库），只报错降级
- [x] 3.7 验证：4 个单元测试覆盖首次建库、重开保数据、损坏重建、未来版本重建

## 4. FileWriteActor 核心

- [x] 4.1 新建 `actor.rs`：定义 `ChangeSet`（file_path、op、base_hash）与 `Request` 枚举
- [x] 4.2 实现 `spawn`：起 tokio task，持有 vault 根路径，返回 `Handle`
- [x] 4.3 实现原子写盘：同目录临时文件 + fsync + rename；**已实测 Windows 上 rename 可覆盖已存在目标**
- [x] 4.4 实现 `append` 操作（不校验 base_hash；读全文补换行再原子写，避免上一行缺换行时黏行）
- [x] 4.5 实现入队：先 INSERT 进 `write_queue`，再唤醒处理
- [x] 4.6 实现处理循环：取队首 → 应用 → 成功则 DELETE 记录 → 广播 `file:changed`
- [x] 4.7 每条 ChangeSet 处理包 `catch_unwind`，单条 panic 不杀 task
- [x] 4.8 sender 发送失败（task 已死）时，command 层作为写入失败上报，不静默丢弃
- [x] 4.9 路径逃逸防护：拒绝绝对路径与含 `..` 的路径（file_path 可能来自外部输入）

## 5. capture 切换到 actor

- [x] 5.1 `capture` 改为构造 append 类型 ChangeSet 并入队，前端签名保持不变
- [x] 5.2 落盘成功后 emit `file:changed`，载荷含 file 与 op；失败 emit `write:failed`，主窗口显示
- [x] 5.3 速记条：入队成功即关窗，不等落盘
- [x] 5.4 验证：连续快速输入 10 条，全部按顺序写入 inbox.md，无丢失无重复
- [x] 5.5 回归延迟测量，确认 p95 未变差

## 6. 冲突检测与 replace_line

- [x] 6.1 加 `blake3` 依赖，实现文件内容哈希
- [x] 6.2 入队时记录 base_hash
- [x] 6.3 实现 `replace_line` 快速路径：哈希匹配则按行号替换，只改该行、其余字节不变
- [x] 6.4 实现慢速路径：哈希不匹配时全文扫描定位 `old_content`
- [x] 6.5 慢速路径匹配到多行时**拒绝并标为失败**，不猜测（见 design D4）
- [x] 6.6 目标行找不到时拒绝并标为失败
- [x] 6.7 实现 `create` 操作：目标已存在则失败
- [x] 6.8 实现整文件替换操作（供后续版本恢复使用）
- [x] 6.9 单元测试：三种操作 × 基线匹配/不匹配/目标消失/多行同内容

（尚未暴露成 command：眼下没有调用方，等便签/编辑器做起来再接。核心逻辑与测试已就位。）

## 7. 重试与失败可见性

- [x] 7.1 实现重试：失败且 retries < 3 时延迟后重试并递增计数
- [x] 7.2 重试耗尽：保留队列记录并标记失败，emit 失败事件
- [x] 7.3 新增 command：查询失败列表（文件路径、操作类型、失败原因）
- [x] 7.4 新增 command：重试指定失败项、丢弃指定失败项
- [x] 7.5 主窗口加失败列表 UI，含重试与丢弃两个操作
- [x] 7.6 验证：把 vault 目录设为只读后速记，看到失败提示；恢复权限后点重试成功写入

## 8. 崩溃恢复

- [x] 8.1 启动时先消费 `write_queue` 中的遗留记录，再接受新请求
- [x] 8.2 实现 append 的重放保护：队列记录存 `applied_marker`，恢复时先比对文件末尾内容再决定是否重放（见 design 的 Risks，append 是唯一不幂等的操作）
- [x] 8.3 验证：写入过程中强杀进程，重启后队列被正确消费，inbox.md 无重复行也无丢失行

## 9. 收尾

- [x] 9.1 `cargo clippy` 无警告
- [x] 9.2 `npm run build` 与 `cargo build` 均通过
- [x] 9.3 全量手动回归：第 1.4 项的全部条目 + 速记写入 + 失败恢复
- [x] 9.4 最终延迟测量，记录 p95 数字
- [x] 9.5 更新 README 的「明确还没做的」一节，移除已完成的 actor 条目
