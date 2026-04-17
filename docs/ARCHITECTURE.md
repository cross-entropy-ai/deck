# deck 架构文档

## 概览

`deck` 当前的结构可以概括为：

- `app.rs`：运行时编排层，负责事件循环、渲染调度、PTY 生命周期和 side effect 执行
- `state.rs`：应用状态与布局计算
- `action.rs`：输入到 `Action` 的映射，以及纯状态变更 `apply_action()`
- `ui.rs`：ratatui 渲染函数
- `refresh.rs`：后台刷新 worker，负责采集 tmux/git 快照
- `tmux.rs` / `git.rs` / `pty.rs` / `config.rs`：外部系统接入层

相比早期版本，两个关键变化已经落地：

- 会阻塞的 session 刷新已经移出 UI 主循环，交给 `RefreshWorker`
- 配置持久化已经改为 `serde` 强类型序列化，而不是手写 JSON

## 模块边界

### `app.rs`

`App` 现在主要承担“壳层”职责：

- 初始化 `Config`、`AppState`、`Pty`、`NestingGuard`、`RefreshWorker`
- 在主循环中处理 PTY 输出、键鼠输入、窗口 resize
- 调用 `action::key_to_action()` / `action::mouse_to_action()`
- 调用 `action::apply_action()` 修改状态
- 执行 `SideEffect`
- 接收并应用 `SessionSnapshot`

`app.rs` 仍然是集成中心，但已经不再自己做整轮同步刷新采集。

### `state.rs`

`AppState` 持有主要运行时状态，包括：

- session 列表、过滤结果、焦点
- 布局模式、view mode、sidebar 尺寸
- 设置页、弹窗、重命名、exclude editor 等 UI 状态
- 插件配置、exclude patterns

同时这里也放布局相关计算，例如：

- sidebar 宽高约束
- PTY 区域尺寸
- 鼠标命中测试

### `action.rs`

这里定义了状态机入口：

- `Action`：用户意图
- `key_to_action()` / `mouse_to_action()`：输入映射
- `apply_action()`：纯状态变更，返回 `SideEffect`

这让大部分交互逻辑可以脱离 `App` 的 IO 上下文单独测试。

### `ui.rs`

`ui.rs` 负责把当前状态渲染成 ratatui 组件，不直接做 IO，也不修改状态。

主要覆盖：

- sidebar
- settings
- theme picker
- exclude editor
- warning / help / context menu

### `refresh.rs`

`RefreshWorker` 在后台线程中执行刷新：

- 接收 `RefreshRequest`
- 调用 tmux / git 采集数据
- 生成 `SessionSnapshot`
- 由 UI 线程通过 `apply_snapshot()` 一次性应用

这样主循环只做“请求刷新”和“应用快照”，不会被慢仓库或慢命令直接阻塞。

### 接入层

- `tmux.rs`：tmux CLI 封装
- `git.rs`：git 状态查询
- `pty.rs`：基于 `portable-pty` 的 PTY 管理
- `bridge.rs`：`vt100` screen 到 ratatui buffer 的适配
- `config.rs`：基于 `serde` 的配置加载与保存
- `nesting_guard.rs`：嵌套 deck/tmux 场景的安全检查

## 运行流程

### 启动

1. `main.rs` 创建 `App`
2. `Config::load()` 读取配置
3. `AppState::new(...)` 初始化状态
4. 启动 tmux PTY
5. 启动 `RefreshWorker`
6. 发送首次刷新请求

### 主循环

每轮循环主要做四件事：

1. 读取 PTY 输出并喂给 `vt100::Parser`
2. 渲染当前 frame
3. 处理键盘、鼠标和 resize 事件
4. 非阻塞接收后台刷新快照并应用

定时刷新时，UI 线程只发请求，不直接执行 tmux/git 查询。

## 当前特征

现状里比较稳定的设计点：

- UI 渲染和状态变更已经分层
- 大多数交互逻辑可以通过 `apply_action()` 单测
- 外部刷新 IO 已经从 UI 主循环拆出
- 配置模型和运行时状态的对应关系比早期版本更清晰

仍然存在的现实情况：

- `app.rs` 仍然是较大的集成文件
- `AppState` 同时包含业务状态和不少 UI 状态
- 一些菜单和集成流程仍然靠集中式分发维护

## 相关文档

- `docs/release.md`
- `docs/todo/decouple-architecture.md`
