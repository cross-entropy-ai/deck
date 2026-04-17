# 解耦方案：分离应用状态与 UI 状态

## 现状分析

app.rs 混合了三个关注点：
- 应用状态（sessions, filtered, filter_mode, session_order）
- UI 状态（layout_mode, theme_index, sidebar_width, show_help, show_borders, context_menu）
- 框架关联（terminal, parser, pty）

## 解耦目标

分离 `AppState`（业务逻辑）和 `UiState`（仅 UI 相关），使应用逻辑可独立于 ratatui 框架进行单元测试。

## 实施步骤

### Phase 1: 状态分离（优先级：🔴 高）

**任务 1.1：创建 AppState 结构体**
- [ ] 创建 `src/state.rs`
- [ ] 定义 `AppState` 包含：
  - `sessions: Vec<SessionRow>`
  - `filtered: Vec<usize>`
  - `focused: usize`
  - `current_session: String`
  - `filter_mode: FilterMode`
  - `session_order: Vec<String>`
  - `theme_index: usize`
- [ ] 实现 `AppState::new()` 初始化方法
- [ ] 实现 `AppState::refresh_sessions()` 等业务逻辑方法

**任务 1.2：创建 UiState 结构体**
- [ ] 在 `src/state.rs` 中定义 `UiState` 包含：
  - `layout_mode: LayoutMode`
  - `sidebar_width: u16`
  - `sidebar_height: u16`
  - `show_help: bool`
  - `show_borders: bool`
  - `confirm_kill: bool`
  - `context_menu: Option<ContextMenu>`
  - `hover_separator: bool`
  - `dragging_separator: bool`
  - `term_width: u16`
  - `term_height: u16`
- [ ] 从 config.rs 加载初始值

**任务 1.3：重构 App 结构体**
- [ ] 修改 `app.rs` 中的 `App` 结构体：
  ```rust
  pub struct App {
      app_state: AppState,
      ui_state: UiState,
      pty: Pty,
      parser: vt100::Parser,
      tick: usize,
      pending_quit: bool,
  }
  ```
- [ ] 更新 `App::new()` 初始化逻辑
- [ ] 编译并确保测试通过

### Phase 2: 事件处理分层（优先级：🟡 中）

**任务 2.1：定义 Action 枚举**
- [ ] 创建 `src/action.rs`
- [ ] 定义 `Action` 枚举表示所有用户可能的操作：
  ```rust
  pub enum Action {
      SwitchSession(String),
      KillSession(String),
      NewSession,
      FilterNext,
      LayoutToggle,
      // ... 更多
  }
  ```

**任务 2.2：分离事件解析**
- [ ] 创建 `fn key_to_action()` 纯函数
- [ ] 创建 `fn mouse_to_action()` 纯函数
- [ ] 从 `handle_key()` 和 `handle_mouse()` 中提取逻辑

**任务 2.3：分离状态更新**
- [ ] 创建 `fn apply_action(state: &mut AppState, action: Action)` 方法
- [ ] 迁移所有状态修改逻辑到此方法

### Phase 3: 验证与优化（优先级：🟢 低）

**任务 3.1：编写单元测试**
- [ ] 为 `AppState` 的业务逻辑编写单测
- [ ] 测试过滤、排序、状态转换
- [ ] 验证测试不需要 ratatui 依赖

**任务 3.2：性能验证**
- [ ] 对比解耦前后的性能
- [ ] 确保没有性能回退

**任务 3.3：代码清理**
- [ ] 删除重复代码
- [ ] 整理模块结构

## 预期收益

| 方面 | 改进 |
|------|------|
| 可测试性 | 业务逻辑可不依赖 ratatui 进行测试 |
| 可维护性 | 减少认知负荷，每个模块职责单一 |
| 可复用性 | AppState 逻辑可用于其他 UI 框架 |
| 代码清晰度 | 数据流更容易追踪 |

## 当前行数估计

- app.rs: ~1300 行 → 可拆分为：
  - state.rs: ~400 行（AppState + UiState）
  - action.rs: ~100 行（Action 枚举 + 处理函数）
  - app.rs: ~500 行（精简版，专注于驱动逻辑）

## 不影响的部分

- ✅ ui.rs（已经是纯函数）
- ✅ tmux.rs、git.rs、pty.rs（已经解耦）
- ✅ bridge.rs（已经是适配层）

## 注意事项

- 重构时保持 git 提交原子性
- 每个 Phase 独立可交付
- Phase 1 完成后即可带来测试收益
