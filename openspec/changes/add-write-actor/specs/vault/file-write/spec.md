## Purpose

规定 vault 内 markdown 文件的唯一写入通道：一个串行的单写者 actor，接受行级 ChangeSet 而非全文提交，保证多个并发写者（编辑器、便签、提醒引擎、速记条、版本恢复）互不覆盖，且进程崩溃后未完成的写入可恢复。

## ADDED Requirements

### Requirement: 单写者独占写权

vault 内所有 markdown 文件的写入必须经由 FileWriteActor 串行执行。除 git 命令外，任何组件不得直接对 vault 内 md 文件调用文件系统写操作。

#### Scenario: 组件请求写入
- **WHEN** 任一组件（速记条、编辑器、便签、提醒引擎、版本恢复）需要修改 md 文件
- **THEN** 该组件提交 ChangeSet 到 actor 队列，由 actor 独占执行实际写盘

#### Scenario: 多个写者同时提交
- **WHEN** 两个以上写者在同一时刻提交针对同一文件的 ChangeSet
- **THEN** actor 按入队顺序串行处理，每次处理前重新读取磁盘当前内容，后处理的变更基于前一次的结果

#### Scenario: git 绕过 actor 写入文件
- **WHEN** `git pull` 或 `git checkout` 由 git 自身写入了 vault 文件
- **THEN** 应用重扫受影响文件刷新索引，并检查各打开缓冲是否与磁盘不一致

### Requirement: 行级 ChangeSet 提交

写者提交的是行级变更描述，不是文件全文。全文提交隐含「提交方持有最新快照」的断言，该断言在并发下不成立，即使串行执行也会丢写。

#### Scenario: 追加内容
- **WHEN** 速记条提交 append 类型的 ChangeSet
- **THEN** actor 将内容追加到目标文件末尾，不校验基线哈希（追加操作天然不与其他变更冲突）

#### Scenario: 替换单行
- **WHEN** 便签或提醒引擎提交 replace_line 类型的 ChangeSet，携带目标行号与该行的原始内容
- **THEN** actor 仅改写该行，文件其余内容逐字节不变

#### Scenario: 编辑器提交全文
- **WHEN** 主编辑器的自动保存被触发
- **THEN** 编辑器提交的是自上次同步以来的 ChangeSet，而非编辑缓冲的全文快照

#### Scenario: 版本恢复整文件覆盖
- **WHEN** 用户从 git 历史恢复某个版本
- **THEN** 允许提交整文件替换（该操作语义本就是整体回退），但仍经由 actor 串行，且执行前强制落盘所有打开缓冲

### Requirement: 基线冲突检测与重定位

ChangeSet 入队时记录目标文件的内容哈希作为基线。actor 处理时若发现磁盘内容已变，须重新定位目标行而非盲目按行号写入。

#### Scenario: 基线未变
- **WHEN** actor 处理 replace_line 且当前文件哈希与 ChangeSet 记录的基线哈希一致
- **THEN** 直接按行号替换

#### Scenario: 基线已变但目标行可定位
- **WHEN** 当前文件哈希与基线不一致，但能在文件中找到与 ChangeSet 记录的原始行内容匹配的行
- **THEN** 按内容匹配到的位置执行替换，忽略原始行号

#### Scenario: 目标行已不存在
- **WHEN** 当前文件哈希与基线不一致，且找不到匹配原始内容的行
- **THEN** 拒绝该 ChangeSet，标记为失败并通知用户，不做任何猜测性写入

### Requirement: 原子写盘

写盘必须保证崩溃或断电不产生半截文件。

#### Scenario: 写入过程中进程被杀
- **WHEN** actor 正在写盘时进程被强制终止
- **THEN** 目标文件保持写入前的完整状态，不出现内容截断或混合

### Requirement: 写队列持久化与崩溃恢复

未完成的 ChangeSet 必须落库，使进程重启后能继续处理。

#### Scenario: ChangeSet 入队
- **WHEN** 写者提交 ChangeSet
- **THEN** 该 ChangeSet 先写入本地数据库队列表，再被处理

#### Scenario: 写盘成功
- **WHEN** actor 成功将某 ChangeSet 落盘
- **THEN** 该记录从队列表删除，并向所有窗口广播该变更

#### Scenario: 进程崩溃后重启
- **WHEN** 应用启动且队列表中存在未完成记录
- **THEN** actor 在处理新请求前先消费这些遗留记录

### Requirement: 失败重试与用户可见性

写失败不得静默丢弃。

#### Scenario: 写入临时失败
- **WHEN** 写盘失败且该 ChangeSet 重试次数少于 3
- **THEN** 短暂延迟后重试，重试计数加一

#### Scenario: 重试耗尽
- **WHEN** 某 ChangeSet 重试 3 次仍失败
- **THEN** 保留该记录在队列中并标记为失败，emit 事件通知前端，主窗口可查看失败列表并手动重试

#### Scenario: 用户查看失败列表
- **WHEN** 用户在主窗口打开写入失败列表
- **THEN** 显示每条失败的文件路径、操作类型与失败原因，并提供重试与丢弃两个操作

### Requirement: 变更广播

写入成功后必须通知所有窗口，使各窗口的视图与磁盘保持一致。

#### Scenario: 便签勾选后主窗口同步
- **WHEN** 便签提交的勾选变更成功落盘，且主编辑器正打开同一文件
- **THEN** 主编辑器收到广播，将该变更映射到自己的编辑状态上，不丢失用户尚未提交的本地修改

#### Scenario: 速记条写入后聚合视图刷新
- **WHEN** 速记条向 inbox.md 追加了一条待办
- **THEN** 待办聚合视图收到广播并刷新，无需用户手动重载
