# Sidebar 键盘导航即时切换

## 现状

- 键盘上下移动（j/k、方向键）只移动 `focused` 光标，不触发 session 切换
- 需要按 Enter (`Action::SwitchProject`) 才真正切换到对应 session
- 存在两种选中状态的视觉区分：绿色（当前 active session）和蓝色（focused/光标所在），造成认知负担

## 目标

1. **即时切换**：键盘上下移动时，立即切换到对应 session，不需要按 Enter 确认
2. **合并选中状态**：去掉蓝色 focused 状态，只保留绿色一种选中样式。移动光标 = 切换 session = 绿色高亮

## 涉及的关键代码

- `src/action.rs`：`Action::MoveDown` / `Action::MoveUp` 目前只修改 `state.focused`，需要同时触发 `fx.switch_session`
- `src/state.rs`：`AppState.focused` 和 `current_session` 的关系需要始终同步
- `src/ui.rs`：渲染逻辑中区分 focused vs active 的颜色判断，合并为一种样式

## 注意事项

- 鼠标点击行为应保持一致：点击即切换（目前已经是这样）
- 快速连续按键时避免频繁触发 tmux switch-client，考虑 debounce
- 搜索模式下移动光标是否也即时切换，需确认（倾向是）
