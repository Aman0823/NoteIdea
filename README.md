# NoteIdea 骨架

笔记本 + 贴屏便签 + 定时提醒。当前是**第一期骨架**，只为验证一件事：

> 热键按下到速记条可输入的真实延迟是否 ≤ 200ms（NFR-2）

这个数字决定 D22（窗口预热）方案是否成立。达不到，一串下游决策都要重写，所以先测再往下写。

需求文档在 `REQUISITES` 之外单独维护，且**不进版本库**（见 `.gitignore`）。

## 需要安装的环境

前端链路已就绪（Node / npm / WebView2 都在）。缺的是 Rust 侧：

### 1. MSVC 构建工具（必装，Tauri 在 Windows 上靠它链接）

下载 [Visual Studio Build Tools](https://visualstudio.microsoft.com/visual-cpp-build-tools/)，安装时勾选：

- **使用 C++ 的桌面开发**
- 确认右侧包含 **MSVC v143 生成工具** 和 **Windows 11 SDK**

约 3-5 GB。只装 Build Tools 就够，不需要完整的 Visual Studio。

### 2. Rust

从 [rustup.rs](https://rustup.rs/) 下载 `rustup-init.exe`，直接默认选项装完即可（会自动选 `x86_64-pc-windows-msvc` 工具链）。

装完**重开一个终端**，然后确认：

```bash
rustc --version   # 应输出 1.77 或更高
cargo --version
```

### 已经具备的（无需操作）

| 组件 | 现状 |
|---|---|
| Node.js | v22.23.2 |
| npm | 10.9.8 |
| WebView2 运行时 | 151.0.4129 |
| git | 2.47.1 |

## 跑起来

```bash
npm install     # 已执行过，依赖在 node_modules
npm run app     # = tauri dev，首次会编译 Rust 依赖，需要几分钟
```

首次 `cargo build` 要拉几百个 crate 并全量编译，慢是正常的；之后增量编译只要几秒。

## 怎么测那个 200ms

1. 应用启动后主窗口会自己打开，说明怎么测。
2. 按 <kbd>Alt</kbd>+<kbd>Space</kbd> 唤出速记条，敲字后 <kbd>Enter</kbd> 存入 `vault/inbox.md`，或 <kbd>Esc</kbd> 取消。
3. **反复唤出十几次**。第一次通常最慢（各种懒加载），之后才是稳态，看 p95 而不是看首次。
4. 回主窗口点「刷新测量结果」，看 p95 是否 ≤ 200ms。
5. 终端里也会实时打印每次的 `[latency] #n 热键→可输入 X ms`。

计时方式：起点是 Rust 侧全局热键回调的第一行（单调时钟），终点是前端聚焦输入框、且**一帧真正绘制完成**后回调 `mark_ready`。用双 `requestAnimationFrame` 确保测的是「用户真的能看见并输入」而不是「JS 执行完了」。

## 顺带可以验证的

- <kbd>Alt</kbd>+<kbd>N</kbd> 唤出主窗口（FR-20）
- 关主窗口只隐藏，进程留在托盘（FR-13）
- 托盘左键 = 速记条，右键菜单可退出
- 再启动一个实例，应聚焦已有实例并弹速记条，而不是开第二个进程（D23）
- 热键被别的软件占用时，主窗口会显示是哪个键失败（FR-21）

## 目录结构

```
src/            前端：main.ts（测量看板）/ quick.ts（速记条）/ styles.css
index.html      主窗口
quick.html      速记条（预热窗口）
src-tauri/      Rust：lib.rs 是全部逻辑，main.rs 只是入口
scripts/        gen-icons.mjs 纯 Node 生成图标，无第三方依赖
vault/          运行期数据，不进 git
```

## 明确还没做的

骨架期刻意留白，避免在没验证性能前堆代码：

- **3.9 的单写者 actor**：`capture` 现在直接写文件。正式实现必须全部走 actor + ChangeSet，这是第一期真正的地基。
- 编辑器（CM6 + decoration）、行内语法解析、SQLite、提醒引擎、便签窗口、git 集成——全部未开始。
- `capture` 无条件加 `- [ ]` 前缀，正式版应由 3.2 的语法解析决定。
- vault 路径硬编码，正式版由用户选择并存 config.json（FR-1）。
