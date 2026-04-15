# deck 架构文档

## 系统架构

```
┌─────────────────────────────────────────┐
│   Terminal UI (ratatui)                 │
│   - ui.rs: 绘制逻辑（纯函数）           │
│   - bridge.rs: vt100 → ratatui 适配     │
└─────────────────┬───────────────────────┘
                  │
        ┌─────────▼──────────┐
        │ Event Handling     │
        │ - Keyboard         │
        │ - Mouse            │
        │ - Resize           │
        └─────────┬──────────┘
                  │
┌─────────────────▼───────────────────────┐
│   Application Logic (app.rs)            │
│   - 状态管理                            │
│   - 事件处理                            │
│   - 渲染驱动                            │
│   - PTY 生命周期                        │
└─────────────────┬───────────────────────┘
                  │
        ┌─────────┴──────────────────────────┐
        │                                    │
    ┌───▼────┐  ┌──────┐  ┌────┐  ┌──────┐ │
    │ tmux   │  │ git  │  │ pty│  │config│ │
    │ (CLI)  │  │(CLI) │  │    │  │(JSON)│ │
    └────────┘  └──────┘  └────┘  └──────┘ │
        Data Layer                          │
    (完全解耦，纯函数或简单命令包装)        │
└─────────────────────────────────────────┘
```

## 核心模块

### 表现层（Presentation）

**ui.rs**
- 纯函数式渲染
- 不持有任何可变状态
- 接收数据结构 `SessionView`，产生 ratatui widgets
- 主要函数：
  - `draw_sidebar()`
  - `draw_sessions()`
  - `draw_help()`
  - `draw_context_menu()`

**bridge.rs**
- vt100 terminal 屏幕 → ratatui buffer 转换
- 处理颜色、样式、宽字符等细节
- 主要函数：
  - `render_screen()`: 逐单元格复制和样式转换
  - `set_cursor()`: 同步光标位置

### 应用层（Application）

**app.rs**（~1300 行）
- 应用状态管理
- 事件循环主驱动
- 当前混合了业务逻辑和 UI 状态

核心数据结构：
```rust
pub struct App {
    // 业务数据
    sessions: Vec<SessionRow>,
    filtered: Vec<usize>,
    focused: usize,
    current_session: String,
    
    // 业务逻辑状态
    session_order: Vec<String>,
    filter_mode: FilterMode,
    
    // UI 状态（不应该在这里）
    layout_mode: LayoutMode,
    theme_index: usize,
    sidebar_width: u16,
    show_help: bool,
    // ...
    
    // 框架关联
    pty: Pty,
    parser: vt100::Parser,
}
```

主要方法：
- `pub fn run()`: 事件循环
- `fn render()`: 绘制帧
- `fn refresh_sessions()`: 定期刷新 tmux 数据
- `fn handle_key()`: 键盘事件处理
- `fn handle_mouse()`: 鼠标事件处理
- `fn apply_filter()`: 应用过滤逻辑

**config.rs**
- 配置文件持久化
- 路径：`~/.config/deck/config.json`
- 可配置项：theme, layout, show_borders, sidebar_width

**theme.rs**
- 颜色主题定义
- 支持多个预设主题

### 数据访问层（Data Layer）

**tmux.rs**（完全解耦）
- 调用 `tmux` CLI 命令
- 无副作用，无状态
- 主要函数：
  - `list_sessions()`: 获取所有会话
  - `current_session()`: 获取当前会话
  - `switch_session()`: 切换会话
  - `new_session()`: 创建会话
  - `kill_session()`: 删除会话

**git.rs**（完全解耦）
- 调用 `git` CLI 命令
- 返回 `GitInfo` 结构体
- 主要函数：
  - `get_git_info(dir)`: 查询指定目录的 git 状态

**pty.rs**（基本解耦）
- 跨平台 PTY 管理
- 使用 `portable_pty` 库
- 提供 `Pty` 结构体和 `PtyEvent` 枚举
- 与 app.rs 的耦合最小

## 数据流

### 初始化流程
```
main.rs
  ↓
App::new()
  ├─ Config::load() → 加载用户偏好
  ├─ tmux::list_sessions() → 初始会话列表
  ├─ Pty::spawn() → 启动 tmux attach PTY
  └─ vt100::Parser::new() → 初始化终端解析器
```

### 事件循环
```
run() 循环：
  1. PTY 读取 → parser.process()
  2. render() → 绘制当前状态
  3. event::poll() → 等待用户输入
  4. 处理事件 → 更新状态
  5. 定期 refresh_sessions() → 从 tmux 拉取最新数据
```

### 渲染流程
```
render() {
  1. 构建 SessionView 切片（借用数据）
  2. terminal.draw(|frame| {
    3. Layout 计算布局区域
    4. ui::draw_sidebar() → 左侧会话列表
    5. bridge::render_screen() → 中央 tmux 输出
    6. 可选：draw_help/draw_context_menu
  })
}
```

## 关键设计决策

### 为什么 SessionView 是借用的？
```rust
pub struct SessionView<'a> {
    pub name: &'a str,      // 借用
    pub branch: &'a str,    // 避免克隆，减少内存压力
    // ...
}
```
- 仅在渲染时存活（frame 内部）
- 避免不必要的 String 克隆

### 为什么 FilterMode 是枚举？
```rust
pub enum FilterMode {
    All,
    Working,
    Idle,
}
```
- 状态机模式，只有有限的合法状态
- 易于循环切换（FilterMode::next()）

### 为什么分离 SessionRow 和 SessionView？
- `SessionRow`: 应用层持有的完整数据
- `SessionView`: UI 层需要的最小数据集
- 提供了基本的解耦

## 耦合分析

### 高耦合（需要改进）
- `app.rs` 混合了应用状态和 UI 状态
- 事件处理和状态更新紧耦合

### 低耦合（现状良好）
- UI 层（ui.rs）是纯函数
- 数据访问层（tmux, git）完全独立
- bridge.rs 是清晰的适配层
- 配置、主题等是独立的

## 可扩展性

### 容易添加的功能
- 新的 UI 主题
- 新的过滤条件
- 新的快捷键
- 新的配置项

### 需要重构才能做的功能
- Web UI 支持（需要分离应用状态）
- 应用逻辑的单元测试（需要分离应用状态）
- 复杂的状态机（需要更清晰的 Action 定义）

## 性能考量

- PTY 输出通过 vt100 解析，每帧成本为 O(cells)
- 会话列表刷新频率：1 秒一次
- Git 查询按需执行（仅在会话列表更新时）
- 键盘轮询间隔：16ms（~60fps）

## 未来改进方向

参见 `todo/decouple-architecture.md`

优先级：
1. 分离 AppState 和 UiState
2. 提取 Action 层
3. 添加单元测试
