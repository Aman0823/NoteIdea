## Purpose

规定 vault 路径的存放与解析方式，以及首次打开 vault 时的目录初始化行为，使应用不再依赖硬编码路径，并保证 vault 内始终存在速记条等功能所需的基础结构。

## ADDED Requirements

### Requirement: 配置文件位置与作用域

vault 路径等跨 vault 的全局设置存放在用户级配置目录，而非 vault 内部。vault 内部的 `.noteidea/` 只存该 vault 自身的派生状态。

#### Scenario: 读取配置
- **WHEN** 应用启动
- **THEN** 从用户级配置目录读取配置，取得上次使用的 vault 路径

#### Scenario: 配置文件不存在
- **WHEN** 应用首次启动，用户级配置文件不存在
- **THEN** 弹出目录选择器要求用户指定 vault 位置，选定后写入配置

#### Scenario: 配置文件损坏
- **WHEN** 配置文件存在但无法解析
- **THEN** 使用默认配置继续启动，并将损坏文件重命名保留，不阻塞应用启动

### Requirement: 首次启动必须由用户指定 vault

应用不擅自决定用户笔记的存放位置。没有可用 vault 时，笔记与速记功能不可用，而不是静默写入某个用户没同意过的目录。

#### Scenario: 用户选定目录
- **WHEN** 首启目录选择器中用户选定了一个目录
- **THEN** 将该路径写入配置，初始化 vault 结构，进入正常可用状态

#### Scenario: 用户取消选择
- **WHEN** 用户关闭目录选择器而未选定任何目录
- **THEN** 应用保持运行（托盘常驻），但速记条与笔记功能处于不可用状态并明确提示原因，提供再次选择的入口

#### Scenario: 未选定 vault 时按下速记热键
- **WHEN** 尚无可用 vault，用户按下速记热键
- **THEN** 提示需要先选择 vault 位置并给出选择入口，不静默丢弃用户输入的内容

### Requirement: vault 路径校验

配置中记录的 vault 路径可能已被用户删除、重命名或位于已断开的移动磁盘上。

#### Scenario: 路径有效且可写
- **WHEN** 配置中的 vault 路径存在且具备写权限
- **THEN** 使用该路径

#### Scenario: 路径已不存在
- **WHEN** 配置中记录的 vault 路径在文件系统上已不存在
- **THEN** 不静默创建同名目录，而是提示用户重新选择 vault 位置

#### Scenario: 路径存在但不可写
- **WHEN** vault 路径存在但无写权限
- **THEN** 明确告知用户路径不可写，不进入「所有写入都失败」的状态

### Requirement: vault 目录初始化

打开一个 vault 时，须确保基础结构存在，缺失的部分补齐。

#### Scenario: 打开空目录作为 vault
- **WHEN** 用户选择一个空目录作为 vault
- **THEN** 创建 `inbox.md`、`.noteidea/` 与 `assets/`，并在 vault 的 `.gitignore` 中加入 `.noteidea/`

#### Scenario: 选定的目录已有非笔记内容
- **WHEN** 用户选定的目录中已存在与 vault 无关的文件
- **THEN** 不清理、不移动任何既有文件，仅补齐基础项

#### Scenario: 打开已有 vault
- **WHEN** 打开一个此前已初始化过的 vault
- **THEN** 保留全部既有内容，仅补齐缺失的基础项，不覆盖任何已存在的文件

#### Scenario: inbox.md 被用户删除
- **WHEN** 用户手动删除了 `inbox.md` 后再次启动应用
- **THEN** 重新创建空的 `inbox.md`，速记条功能保持可用

#### Scenario: vault 不是 git 仓库
- **WHEN** 选定的 vault 目录不是 git 仓库
- **THEN** 询问用户是否执行 git 初始化，用户拒绝时 vault 仍可正常使用，仅 git 相关功能不可用

### Requirement: 派生数据与用户数据分离

`.noteidea/` 下的一切都必须是可丢弃的派生数据。

#### Scenario: 删除整个 .noteidea 目录后启动
- **WHEN** 用户删除 vault 内的 `.noteidea/` 目录后启动应用
- **THEN** 应用重建该目录并重扫 vault，用户的笔记内容与提醒配置一条不少（因其全部存于 md 中），仅丢失提醒的已触发记录与便签摆放位置
