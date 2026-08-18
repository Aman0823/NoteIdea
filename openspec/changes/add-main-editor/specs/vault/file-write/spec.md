## MODIFIED Requirements

### Requirement: 行级 ChangeSet 提交

写者提交的是变更描述，不是文件全文。全文提交隐含「提交方持有最新快照」的断言，该断言在并发下不成立，即使串行执行也会丢写。

变更描述有两种坐标系：行级（`append` / `replace_line`）与字符偏移（`apply_edits`）。主编辑器用后者，其余写者用前者。整文件替换仅版本恢复可用。

#### Scenario: 追加内容
- **WHEN** 速记条提交 append 类型的 ChangeSet
- **THEN** actor 将内容追加到目标文件末尾，不校验基线哈希（追加操作天然不与其他变更冲突）

#### Scenario: 替换单行
- **WHEN** 便签或提醒引擎提交 replace_line 类型的 ChangeSet，携带目标行号与该行的原始内容
- **THEN** actor 仅改写该行，文件其余内容逐字节不变

#### Scenario: 编辑器提交字符偏移变更
- **WHEN** 主编辑器的自动保存被触发
- **THEN** 编辑器提交 apply_edits 类型的 ChangeSet，携带一组 `{from, to, insert}` 与基线哈希，偏移相对基线所指的那份内容；不得提交编辑缓冲的全文快照

#### Scenario: 一次提交含多处不相邻改动
- **WHEN** 用户在文件开头和结尾各改了一处，两处之间大量内容未改
- **THEN** ChangeSet 只携带这两处改动，不得把它们之间未改动的内容一并塞进 payload

#### Scenario: 版本恢复整文件覆盖
- **WHEN** 用户从 git 历史恢复某个版本
- **THEN** 允许提交整文件替换（该操作语义本就是整体回退），但仍经由 actor 串行，且执行前强制落盘所有打开缓冲

### Requirement: 基线冲突检测与重定位

ChangeSet 入队时记录目标文件的内容哈希作为基线。actor 处理时若发现磁盘内容已变，行级 op 须重新定位目标行而非盲目按行号写入；字符偏移 op 不得重定位，一律拒绝。

#### Scenario: 基线未变
- **WHEN** actor 处理 replace_line 且当前文件哈希与 ChangeSet 记录的基线哈希一致
- **THEN** 直接按行号替换

#### Scenario: 基线已变但目标行可定位
- **WHEN** 当前文件哈希与基线不一致，但能在文件中找到与 ChangeSet 记录的原始行内容匹配的行
- **THEN** 按内容匹配到的位置执行替换，忽略原始行号

#### Scenario: 目标行已不存在
- **WHEN** 当前文件哈希与基线不一致，且找不到匹配原始内容的行
- **THEN** 拒绝该 ChangeSet，标记为失败并通知用户，不做任何猜测性写入

#### Scenario: 字符偏移变更遇到基线不一致
- **WHEN** actor 处理 apply_edits 且当前文件哈希与基线哈希不一致
- **THEN** 拒绝该 ChangeSet 并回报当前磁盘内容的哈希，不得按原偏移写入——磁盘上任何长度变化都会使全部后续偏移错位，按错位偏移写入是静默的内容损坏

#### Scenario: 字符偏移超出文件长度
- **WHEN** apply_edits 中某个 `to` 大于当前文件内容长度
- **THEN** 拒绝整个 ChangeSet，不得截断或部分应用

#### Scenario: 一批偏移变更中途失败
- **WHEN** apply_edits 携带多处变更，其中任一处校验不通过
- **THEN** 整批都不应用，文件保持处理前状态，不得留下应用了一半的中间态

### Requirement: 变更广播

actor 落盘成功后必须向所有窗口广播该次变更，使各窗口能把自己尚未提交的本地变更在新基线上重映射。

#### Scenario: 落盘成功后广播
- **WHEN** actor 成功将某 ChangeSet 落盘
- **THEN** 向所有窗口广播该 ChangeSet 的内容与目标文件路径

#### Scenario: 广播内容足以让前端重映射
- **WHEN** 窗口收到广播
- **THEN** 广播携带的信息足以在该窗口的文档坐标系里还原出等价的变更（行级 op 需能换算成字符偏移），否则重映射无从进行

#### Scenario: 提交方收到自己那批变更的广播
- **WHEN** 广播的 ChangeSet 正是本窗口此前提交的那一批
- **THEN** 本窗口将其标记为已确认并从未确认变更中移除，不得重复应用一遍
