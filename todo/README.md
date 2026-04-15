# deck TODO

## 架构

| 文件 | 优先级 | 复杂度 | 状态 |
|------|--------|--------|------|
| [decouple-architecture.md](decouple-architecture.md) | 🔴 高 | 中 | 待开始 |
| [agent-token-usage-display.md](agent-token-usage-display.md) | 🔴 高 | 中 | 框架已写 |
| [AGENT_METRICS_IMPLEMENTATION_GUIDE.md](AGENT_METRICS_IMPLEMENTATION_GUIDE.md) | — | — | 参考文档 |

## UI 改进

按建议实施顺序排列：

### 已完成

| # | 文件 | 内容 | 完成时间 |
|---|------|------|----------|
| 04 | [ui-04-idle-time-format.md](ui-04-idle-time-format.md) | idle 时长展示（左侧 badge，`1m / 2h / 3d`） | 2026-04-15 |
| 05 | [ui-05-git-status-symbols.md](ui-05-git-status-symbols.md) | git status 符号化（`↑2 ↓1 +3 ~1 ?2` / `✓`） | 2026-04-15 |
| 08 | [ui-08-separator-visual.md](ui-08-separator-visual.md) | 分隔线拖拽视觉增强 | 2026-04-15 |
| 06 | [ui-06-session-rename.md](ui-06-session-rename.md) | 会话重命名（r 键 inline 编辑） | 2026-04-15 |
| 02 | [ui-02-compact-mode.md](ui-02-compact-mode.md) | 紧凑模式（2行/card vs 5行/card，Settings + `c` 键切换） | 2026-04-16 |

### 下一批（中等成本，高价值）

| # | 文件 | 内容 | 复杂度 |
|---|------|------|--------|
| 03 | [ui-03-scroll-indicator.md](ui-03-scroll-indicator.md) | 滚动指示器（▲ 2 more / 3/10） | 低 |
| 01 | [ui-01-fuzzy-search.md](ui-01-fuzzy-search.md) | 模糊搜索（/ 键搜索 session） | 中 |
| 07 | [ui-07-toast-notifications.md](ui-07-toast-notifications.md) | 操作反馈 toast 通知 | 低 |

### 可选（高成本或低价值）

| # | 文件 | 内容 | 复杂度 |
|---|------|------|--------|
| 09 | [ui-09-command-palette.md](ui-09-command-palette.md) | Command Palette（Ctrl+P 命令搜索） | 高 |
| 11 | [ui-11-auto-theme-detection.md](ui-11-auto-theme-detection.md) | 主题自动跟随终端 light/dark | 中 |
| 12 | [ui-12-responsive-layout.md](ui-12-responsive-layout.md) | 响应式布局阈值 | 中 |

### 远期

| # | 文件 | 内容 | 复杂度 |
|---|------|------|--------|
| 10 | [ui-10-session-preview.md](ui-10-session-preview.md) | Session Preview 画中画 | 高 |

## 依赖关系

```
ui-04 (idle time) ──┐
                    ├──→ ui-02 (compact mode) ✅ ──→ ui-09 (command palette)
ui-05 (git symbols)─┘                                  ↑
                                                        │
                           ui-06 (rename) ✅ ───────────┘
```

- ui-02 (compact mode) 和 ui-06 (rename) 均已完成
- command palette 的两个前置项已满足，可以开始
- 其他改进之间相互独立
