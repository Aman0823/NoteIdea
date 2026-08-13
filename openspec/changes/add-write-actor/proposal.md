## Why

骨架期 `capture` 命令直接 `fs::write` 写 `inbox.md`。但按 REQUIREMENTS.md 的 3.9，同一个 md 文件有五个潜在写者（主编辑器、便签窗口、提醒引擎、速记条、版本恢复），无仲裁必然丢写和损坏文件。

这不是「以后加个锁」能补的东西——一旦编辑器和便签都各自直写文件，后续每个并发 bug 都要靠打补丁修，而每个补丁又制造新的竞态。第一期必须先把这层地基铺对，其余四个写者才有地方挂。

现在动手的时机也对：骨架已验证 p95 = 27ms 远低于 NFR-2 的 200ms 门槛，性能风险已排除，可以放心加异步写入层。

## What Changes

- 新增 `FileWriteActor`：跑在独立 tokio task 的单写者，串行消费写队列，是**唯一**允许触碰 vault 内 md 文件的组件
- 新增 ChangeSet 协议：所有写者提交行级变更而非全文（全文提交隐含「我这份快照最新」的错误断言，即使串行也会丢写）
- 新增 SQLite 持久化（`.noteidea/local.db`）：写队列落库，进程崩溃重启后未完成的写入自动恢复
- 新增 config 层：vault 路径从硬编码改为读配置，首次启动初始化 vault 目录结构
- **BREAKING**（仅内部）：`capture` 命令从同步写改为入队。前端调用签名不变，但返回时写盘尚未完成，成功与否靠事件通知
- 写失败重试 3 次，仍失败则保留队列并 emit 事件，主窗口可见失败列表

## Capabilities

### New Capabilities

- `vault/file-write`: 单写者 actor 与 ChangeSet 写入协议。规定谁能写 md、提交什么粒度、冲突如何判定、失败如何恢复
- `vault/config`: vault 路径解析与目录初始化。规定配置存放位置、首次启动行为、目录结构约定
- `vault/persistence`: SQLite 本地状态库。规定 schema、职责边界（只存派生数据）、损坏时的重建策略

### Modified Capabilities

<!-- 无。骨架期没有任何已归档的 spec，全部为新建。 -->

## Impact

**代码**
- `src-tauri/src/lib.rs`：`capture` 改为入队；`setup()` 增加 config 加载、DB 初始化、actor 启动
- 新增 `src-tauri/src/config.rs`、`db.rs`、`actor.rs`（当前全部逻辑挤在 lib.rs，借这次拆模块）

**依赖**（新增 Cargo）
- `rusqlite` (bundled)：本地库，bundled 避免要求用户装 SQLite
- `tokio`：actor task 与异步文件 IO
- `blake3`：文件内容哈希，用于 ChangeSet rebase 判定
- `anyhow`：错误传递

**数据**
- vault 内新增 `.noteidea/local.db`，需进 `.gitignore`（`.noteidea/` 已在忽略列表）

**不受影响**
- 前端代码零改动。`invoke('capture', { text })` 签名不变
- 托盘、热键、窗口预热、单实例逻辑不动
- NFR-2 的 27ms 不应变差（入队是异步的，不阻塞热键路径），但需回归验证
