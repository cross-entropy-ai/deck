---
title: 被动探测本机 Claude Code 会话（pid → session → ai-title）
created: 2026-06-03
tags: [claude-code, probe, macos, reverse-engineering, session]
status: validated
platform: macOS（无 root / 无 dtrace / 无 fs_usage）
caveat: 依赖 Claude Code 未文档化的内部文件，升级可能失效，仅作 best-effort 探测
---

# 被动探测本机 Claude Code 会话

## 目标

在一台机器上**被动**地（不修改任何东西、不依赖任何进程/用户配合、无需 root）枚举出
当前所有运行中的 Claude Code CLI 进程，并对每个进程给出：

- 它属于哪个 session（`sessionId`）
- 该 session 的人类可读标题（自动生成的 `ai-title`，即 `/resume` 列表里看到的那行）

## 背景：相关文件与格式

### transcript（会话记录）
- 路径：`~/.claude/projects/<cwd 编码>/<session-id>.jsonl`
- 格式：**JSONL**，一行一个 JSON 对象，append-only。
- 行用 `type` 区分：`user` / `assistant` / `attachment` / `system` / `mode` /
  `last-prompt` / `ai-title` / `queue-operation` / `file-history-snapshot`。
- `ai-title` 行携带 `{"type":"ai-title","aiTitle":"...","sessionId":"..."}`，
  随对话推进**反复重写**——取最后一条。
- 对话靠 `uuid` + `parentUuid` 串成树（支持分叉 / sidechain，不是纯线性数组）。
- 一个 assistant 回合会拆成多行：`thinking` / `text` / `tool_use` 各自落行，
  工具结果作为下一条 `user` 行回灌（带 `toolUseResult`）。

### 运行时状态文件（关键）
- 路径：`~/.claude/sessions/<PID>.json` —— **Claude Code 给每个运行进程写的、以 PID 命名的状态文件**。
- 字段：`pid` / `sessionId` / `cwd` / `version` / `status` /
  `procStart`（人类可读串）/ `startedAt`（**epoch 毫秒**）/ `updatedAt` /
  `entrypoint` / `kind` / `bridgeSessionId` / `peerProtocol`。
- ⚠️ **不含 title**——但有 `sessionId`，足以定位 transcript 再读标题。

## 走过的弯路（为什么这些方法不行）

| 方法 | 结果 | 原因 |
|---|---|---|
| `pgrep claude` + 看 cwd | 抓错进程 | 同名进程多个；`pgrep -n` 取最新启动的，不一定是目标 |
| `lsof -p <pid>` 看打开的 transcript | idle 时拿不到 | claude 不长开 transcript，open→append→close |
| 高频 `lsof` 采样（~6s）抓写入瞬间 | 连自己都没抓到 | 单次 lsof ~50ms+，open 窗口比采样间隔还短 |
| 读 claude 进程自身环境变量 | 没有 session id | `CLAUDE_CODE_SESSION_ID` 只注入**子进程**（Bash 工具 / hook），不在 claude 进程本身 |
| 按 cwd 取目录里**最近修改**的 `.jsonl` | **串台** | 同一 cwd 并发多个会话时，最新文件可能是别的会话 |

最后可用的信号：**`~/.claude/sessions/<PID>.json`**。

## Codex 评审 + 本机实测验证

把方案交给 Codex 评审，它点出的关键问题都在本机得到验证：

### ① PID 复用是最大正确性风险（验证：真，且有时区坑）
`kill -0 <pid>` 只证明"这 PID 有进程"，不证明是同一个 claude——PID 会被复用。必须交叉校验启动时间。

实测发现 `procStart` 那个**人类可读串是 UTC，`ps lstart` 是本地时区**，差 8 小时却是同一时刻：

```
pid=33448  session.json procStart = Wed Jun  3 03:46:47 2026   ← UTC
           ps lstart            = Wed  3 Jun 11:46:47 2026     ← 本地 +08
```

→ **别用 `procStart` 字符串比**。用 epoch 字段 `startedAt`（毫秒，无时区歧义）对
`ps` 启动时间换算后比，容差几秒。

### ② cwd→目录编码不可逆（验证：比 Codex 想的更糟）
编码把 **`/` 和 `.` 都映射成 `-`**：

```
/Users/junyi/.ssh   →  -Users-junyi--ssh
-Users-junyi-claude-ascend-ascend-inference   ← 无法反推分隔点在哪
```

→ **绝不靠 cwd 反算 transcript 路径**。

### ③ 修正：按 sessionId 全局 glob（验证：sessionId 全局唯一）
本机所有 `.jsonl` 的 basename 去重检查无重复 → `sessionId` 全局唯一 →

```bash
find ~/.claude/projects -name "<sessionId>.jsonl"
```

精确命中，绕开 lossy 编码。恰好一个才用，多个→标 ambiguous。

### ④ "no transcript" 是合法的一等状态
缺 transcript 不等于探测失败，可能是：新会话还没落盘 / 旧版本（2.1.117）/
headless / subagent / 文件被删移。→ 显式标 `no-transcript` 或 `no-ai-title`，
**绝不退化成"按 cwd 取最新文件"**（已知会串台）。

### ⑤ 置信度模型
每条结果带置信度 + 证据，而不是只给标题：

- `high`：活 claude 进程 + PID/启动时间吻合 + sessionId 精确/唯一命中 + 有标题
- `medium`：进程吻合，但 transcript 仅 glob 命中 / 标题缺失 / 启动时间没法比
- `low`：liveness 过，但 sessionId 命中多个（ambiguous）
- `reject`：PID 复用 / 非 claude 进程 / PID 与文件名不符

## 最终算法

1. 遍历 `~/.claude/sessions/*.json`，取 `pid / sessionId / startedAt / version`。
2. **防复用**：`kill -0 $pid` 且文件名 PID == json `pid`；进程 comm 是 claude/node；
   `ps` 启动时间换算后 ≈ `startedAt`（epoch 比较，容差几秒）。不过则 `reject`，
   时间没法解析则降级 `medium`。
3. **定位 transcript**：`find ~/.claude/projects -name "$sid.jsonl"`（glob，不反算路径）。
   恰一个用之，零个 `no-transcript`，多个 `ambiguous`。
4. **读标题**：防御式解析，取最后一条合法 `ai-title.aiTitle`；空则 `no-ai-title`。
5. 输出带置信度。

## 参考实现（已在 macOS bash 3.2 + jq 跑通）

```bash
#!/usr/bin/env bash
# claude-probe — 被动探测本机所有运行中的 Claude Code 会话及其 ai-title。
# 只读 ~/.claude 下的内部状态文件，不修改任何东西，不依赖任何配合。
set -u
SESS_DIR="$HOME/.claude/sessions"
PROJ_DIR="$HOME/.claude/projects"
TOL=5   # startedAt 与进程启动时间允许的误差秒数

epoch_of_pid() {  # 取活进程的启动 epoch 秒；解析失败回显空
  local pid=$1 ls
  ls=$(ps -p "$pid" -o lstart= 2>/dev/null) || return
  [ -z "$ls" ] && return
  date -j -f "%a %b %e %T %Y" "$ls" +%s 2>/dev/null \
    || date -j -f "%a %e %b %T %Y" "$ls" +%s 2>/dev/null
}

printf '%-7s %-9s %-8s %-13s %-10s %s\n' PID CONF VER STATE SESSION TITLE
printf '%-7s %-9s %-8s %-13s %-10s %s\n' ----- ---- --- ----- ------- -----

for f in "$SESS_DIR"/*.json; do
  [ -e "$f" ] || continue
  pid=$(jq -r '.pid // empty' "$f")
  sid=$(jq -r '.sessionId // empty' "$f")
  ver=$(jq -r '.version // "?"' "$f")
  started=$(jq -r '.startedAt // empty' "$f")
  fname_pid=$(basename "$f" .json)

  conf=high; state=ok

  if ! kill -0 "$pid" 2>/dev/null; then conf=reject; state=dead
  elif [ "$pid" != "$fname_pid" ]; then conf=reject; state=pidmismatch
  else
    comm=$(ps -p "$pid" -o comm= 2>/dev/null)
    case "$comm" in *claude*|*node*) : ;; *) conf=reject; state=notclaude ;; esac
  fi
  if [ "$conf" != reject ] && [ -n "$started" ]; then
    live=$(epoch_of_pid "$pid")
    if [ -n "$live" ]; then
      want=$(( started / 1000 )); d=$(( live > want ? live-want : want-live ))
      [ "$d" -gt "$TOL" ] && { conf=reject; state=reused; }
    else
      [ "$conf" = high ] && conf=medium
    fi
  fi

  title=""
  if [ "$conf" != reject ] && [ -n "$sid" ]; then
    n=0; first=""
    while IFS= read -r line; do n=$((n+1)); [ -z "$first" ] && first="$line"; done \
      < <(find "$PROJ_DIR" -name "$sid.jsonl" 2>/dev/null)
    if   [ "$n" -eq 0 ]; then state=no-transcript; [ "$conf" = high ] && conf=medium
    elif [ "$n" -gt 1 ]; then state=ambiguous;     conf=low
    else
      title=$(jq -r 'select(.type=="ai-title") | .aiTitle // empty' "$first" 2>/dev/null | tail -1)
      [ -z "$title" ] && { state=no-ai-title; [ "$conf" = high ] && conf=medium; }
    fi
  fi

  printf '%-7s %-9s %-8s %-13s %-10s %s\n' \
    "$pid" "$conf" "$ver" "$state" "${sid:0:8}" "${title:-—}"
done
```

### 实测输出样例

```
PID     CONF      VER      STATE         SESSION    TITLE
-----   ----      ---      -----         -------    -----
33448   high      2.1.161  ok            e849f4f3   Understand tpu_vm recover vs wait-ready
47864   high      2.1.160  ok            c6230cbe   Audio transcription project setup
52775   reject    2.1.117  reused        978f684c   —
53361   medium    2.1.117  no-transcript d57bf68f   —
56786   high      2.1.160  ok            4bc2cdd8   Auto-close settings window on section focus
74768   high      2.1.160  ok            68fb36e9   Debug GCS cache reading environment
94505   medium    2.1.161  no-transcript 350109da   —
```

注意 `52775` 被判 `reject/reused`——startedAt 与进程启动时间对不上，防护正确拦下，
没把陈旧/复用 PID 的标题当真。

## 风险与边界

- 整套依赖 **未文档化的内部文件**（`sessions/<pid>.json`、`ai-title` 字段、目录编码），
  Claude Code 一升级就可能变。**只能当 best-effort 探测，不能当可靠 API**。
- 必须自带交叉校验 + 置信度，绝不输出裸标题。
- 不要再花时间去抓 transient 文件 open（lsof 采样）——窗口太短，`sessions/<pid>.json`
  是远更强的信号。

## 附：自查当前会话

```bash
echo $CLAUDE_CODE_SESSION_ID                          # 当前会话 id（子进程环境里有）
f=$(find ~/.claude/projects -name "$CLAUDE_CODE_SESSION_ID.jsonl")
jq -r 'select(.type=="ai-title").aiTitle' "$f" | tail -1   # 当前会话标题
```

