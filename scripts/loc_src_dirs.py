#!/usr/bin/env python3
"""Stacked-area breakdown of code line count by top-level module over time.

Every Rust file under src/ at each day's last commit is bucketed by its
top-level directory (app, ui, infra, model, session, system) — files sitting
directly in src/ go to "root". The whole tests/ tree is bucketed into a single
"tests" band on top. (Inline #[cfg(test)] modules inside src/ stay folded into
their owning module — only the standalone tests/ tree is split out.) Drawn as a
stacked area so you can see how the weight of each module shifts as the
codebase grows.
"""

import re
import subprocess
from collections import OrderedDict, defaultdict
from datetime import datetime

import matplotlib.pyplot as plt
import matplotlib.dates as mdates

# subfolder -> color (distinct hues, matched to the repo's other LOC charts)
CATS = ["app", "ui", "infra", "model", "session", "system", "root", "tests"]
COLORS = {
    "app":     "#d96846",   # terracotta
    "ui":      "#26a69a",   # teal
    "infra":   "#5c80bc",   # slate blue
    "model":   "#e3a857",   # amber
    "session": "#9b6a9e",   # mauve
    "system":  "#6a994e",   # green
    "root":    "#b0b0b0",   # grey (main.rs, cli.rs, ...)
    "tests":   "#7a7a7a",   # dark grey (the standalone tests/ tree)
}


def run(args):
    return subprocess.run(args, capture_output=True, text=True, check=True).stdout


def categorize(path):
    """src/app/foo.rs -> app ; src/main.rs -> root ; tests/**.rs -> tests."""
    if path.startswith("tests/"):
        return "tests"
    rest = path[len("src/"):]
    if "/" in rest:
        top = rest.split("/", 1)[0]
        return top if top in COLORS else "root"
    return "root"


def day_commits():
    """date -> sha of that day's last commit (oldest day first)."""
    out = run(["git", "log", "--reverse", "--format=%H %cI"])
    by_day = OrderedDict()
    for line in out.splitlines():
        sha, iso = line.split(" ", 1)
        by_day[datetime.fromisoformat(iso).date()] = sha  # keep last seen
    return by_day


def breakdown(sha):
    """subfolder -> line count across src/**/*.rs and tests/**/*.rs at this commit."""
    res = subprocess.run(["git", "grep", "-I", "-c", "", sha, "--",
                          "src/*.rs", "tests/*.rs"],
                         capture_output=True, text=True)
    acc = defaultdict(int)
    for line in res.stdout.splitlines():
        # format: <sha>:<path>:<count>
        rest = line.split(":", 1)[1]
        path, cnt = rest.rsplit(":", 1)
        acc[categorize(path)] += int(cnt)
    return acc


def release_tags():
    """[(date, name)] for minor releases + latest, one label per day."""
    out = run(["git", "tag", "--sort=creatordate",
               "--format=%(refname:short) %(creatordate:iso-strict)"])
    tags = []
    for line in out.splitlines():
        name, iso = line.split(" ", 1)
        tags.append((datetime.fromisoformat(iso).date(), name))
    if not tags:
        return tags
    keep = [(d, n) for d, n in tags if re.fullmatch(r"v\d+\.\d+\.0", n)]
    keep.append(tags[-1])
    per_day = OrderedDict()
    for d, n in keep:
        per_day[d] = n
    return list(per_day.items())


def main():
    plt.rcParams.update({
        "font.family": "sans-serif",
        "font.sans-serif": ["Helvetica Neue", "Helvetica", "Arial", "DejaVu Sans"],
        "font.size": 11,
        "axes.linewidth": 0.8,
        "axes.edgecolor": "#444444",
        "svg.fonttype": "none",
    })

    by_day = day_commits()
    dates = list(by_day.keys())
    series = {c: [] for c in CATS}
    for sha in by_day.values():
        acc = breakdown(sha)
        for c in CATS:
            series[c].append(acc.get(c, 0))

    total0 = sum(series[c][0] for c in CATS)
    total1 = sum(series[c][-1] for c in CATS)
    print(f"{len(dates)} days, {total0} -> {total1} lines (src/ + tests/)")
    for c in CATS:
        pct = 100 * series[c][-1] / total1 if total1 else 0
        print(f"  {c:<8} {series[c][0]:>6} -> {series[c][-1]:>6}  ({pct:4.1f}%)")

    fig, ax = plt.subplots(figsize=(13, 6.5), dpi=200)
    ax.stackplot(dates, [series[c] for c in CATS],
                 labels=CATS, colors=[COLORS[c] for c in CATS],
                 edgecolor="white", linewidth=0.3)

    ax.set_title("deck — code composition by module over time (src/ + tests/)",
                 fontsize=14, fontweight="bold", pad=30, loc="left")
    ax.set_ylabel("Lines of code (stacked)", fontsize=12)
    ax.set_xlabel("Date (2026)", fontsize=12)
    ax.grid(True, axis="y", color="#000000", alpha=0.06, lw=0.8)
    ax.set_axisbelow(True)
    for s in ("top", "right"):
        ax.spines[s].set_visible(False)
    ax.margins(x=0, y=0)
    ax.set_ylim(0, total1 * 1.12)

    # release tags
    ymax = total1 * 1.12
    for day, name in release_tags():
        x = mdates.date2num(day)
        ax.axvline(x, color="#37474f", lw=0.7, ls=(0, (4, 3)),
                   alpha=0.4, zorder=4)
        ax.annotate(name, xy=(x, ymax), xytext=(0, 4),
                    textcoords="offset points", rotation=90,
                    ha="center", va="bottom", fontsize=8,
                    color="#37474f", annotation_clip=False)

    # legend with final-day percentage share next to each label
    handles, labels = ax.get_legend_handles_labels()
    pct_labels = [f"{c}  ({100 * series[c][-1] / total1:.0f}%)" for c in CATS]
    ax.legend(handles[::-1], pct_labels[::-1], loc="upper left",
              fontsize=10, frameon=False, title="module (latest share)",
              bbox_to_anchor=(1.005, 1.0), borderaxespad=0)

    ax.xaxis.set_major_locator(mdates.WeekdayLocator(byweekday=mdates.MO))
    ax.xaxis.set_major_formatter(mdates.DateFormatter("%b %d"))
    ax.xaxis.set_minor_locator(mdates.DayLocator())

    fig.subplots_adjust(left=0.07, right=0.84, top=0.9, bottom=0.1)
    out = "scripts/loc_src_dirs.png"
    fig.savefig(out)
    print(f"saved {out}")


if __name__ == "__main__":
    main()
