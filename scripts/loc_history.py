#!/usr/bin/env python3
"""Render repository and Rust-module line-count history charts.

By default both views are generated. Use ``--view repo`` or
``--view modules`` when only one chart is needed.
"""

import argparse
import re
import subprocess
from collections import OrderedDict, defaultdict
from datetime import datetime

import matplotlib.dates as mdates
import matplotlib.pyplot as plt


REPO_CATEGORIES = ["Rust", "docs", "config", "other"]
MODULE_CATEGORIES = ["app", "ui", "infra", "model", "session", "system", "root", "tests"]
COLORS = {
    "Rust": "#d96846",
    "docs": "#26a69a",
    "config": "#5c80bc",
    "other": "#b0b0b0",
    "app": "#d96846",
    "ui": "#26a69a",
    "infra": "#5c80bc",
    "model": "#e3a857",
    "session": "#9b6a9e",
    "system": "#6a994e",
    "root": "#b0b0b0",
    "tests": "#7a7a7a",
}
CONFIG_EXTENSIONS = {
    ".toml", ".yaml", ".yml", ".json", ".lock", ".ini", ".cfg",
    ".conf", ".gitignore", ".gitattributes", ".editorconfig",
}
DOC_EXTENSIONS = {".md", ".markdown", ".rst", ".txt", ".adoc"}
VIEWS = {
    "repo": {
        "categories": REPO_CATEGORIES,
        "title": "deck — codebase composition over time",
        "output": "scripts/loc_stacked.png",
        "legend": "stack order",
    },
    "modules": {
        "categories": MODULE_CATEGORIES,
        "title": "deck — code composition by module over time (src/ + tests/)",
        "output": "scripts/loc_src_dirs.png",
        "legend": "module (latest share)",
    },
}


def run(args):
    return subprocess.run(args, capture_output=True, text=True, check=True).stdout


def repo_category(path):
    lower = path.lower()
    if lower.endswith(".rs"):
        return "Rust"
    if lower.startswith("docs/") or any(lower.endswith(ext) for ext in DOC_EXTENSIONS):
        return "docs"
    name = lower.rsplit("/", 1)[-1]
    dot = name.rfind(".")
    extension = name[dot:] if dot >= 0 else name
    if extension in CONFIG_EXTENSIONS or name in CONFIG_EXTENSIONS or name.startswith("."):
        return "config"
    return "other"


def module_category(path):
    if path.startswith("tests/") and path.endswith(".rs"):
        return "tests"
    if not path.startswith("src/") or not path.endswith(".rs"):
        return None
    rest = path[len("src/"):]
    if "/" not in rest:
        return "root"
    top = rest.split("/", 1)[0]
    return top if top in MODULE_CATEGORIES else "root"


def day_commits():
    """Return date -> SHA for each day's last commit, oldest first."""
    commits = OrderedDict()
    for line in run(["git", "log", "--reverse", "--format=%H %cI"]).splitlines():
        sha, iso = line.split(" ", 1)
        commits[datetime.fromisoformat(iso).date()] = sha
    return commits


def breakdown(sha, categorize):
    """Count tracked text lines at ``sha`` using the selected categorizer."""
    result = subprocess.run(
        ["git", "grep", "-I", "-c", "", sha], capture_output=True, text=True
    )
    counts = defaultdict(int)
    for line in result.stdout.splitlines():
        rest = line.split(":", 1)[1]
        path, count = rest.rsplit(":", 1)
        category = categorize(path)
        if category is not None:
            counts[category] += int(count)
    return counts


def release_tags():
    """Return minor releases plus the latest tag, with one label per day."""
    tags = []
    output = run([
        "git", "tag", "--sort=creatordate",
        "--format=%(refname:short) %(creatordate:iso-strict)",
    ])
    for line in output.splitlines():
        name, iso = line.split(" ", 1)
        tags.append((datetime.fromisoformat(iso).date(), name))
    if not tags:
        return []
    selected = [(day, name) for day, name in tags if re.fullmatch(r"v\d+\.\d+\.0", name)]
    selected.append(tags[-1])
    return list(OrderedDict(selected).items())


def configure_plot_style():
    plt.rcParams.update({
        "font.family": "sans-serif",
        "font.sans-serif": ["Helvetica Neue", "Helvetica", "Arial", "DejaVu Sans"],
        "font.size": 11,
        "axes.linewidth": 0.8,
        "axes.edgecolor": "#444444",
        "svg.fonttype": "none",
    })


def render(view_name, commits):
    spec = VIEWS[view_name]
    categories = spec["categories"]
    categorize = repo_category if view_name == "repo" else module_category
    dates = list(commits)
    series = {category: [] for category in categories}
    for sha in commits.values():
        counts = breakdown(sha, categorize)
        for category in categories:
            series[category].append(counts.get(category, 0))

    initial = sum(series[category][0] for category in categories)
    latest = sum(series[category][-1] for category in categories)
    print(f"{view_name}: {len(dates)} days, {initial} -> {latest} lines")
    for category in categories:
        share = 100 * series[category][-1] / latest if latest else 0
        print(f"  {category:<8} {series[category][0]:>6} -> {series[category][-1]:>6}  ({share:4.1f}%)")

    fig, ax = plt.subplots(figsize=(13, 6.5), dpi=200)
    ax.stackplot(
        dates,
        [series[category] for category in categories],
        labels=categories,
        colors=[COLORS[category] for category in categories],
        edgecolor="white",
        linewidth=0.3,
    )
    ax.set_title(spec["title"], fontsize=14, fontweight="bold", pad=30, loc="left")
    ax.set_ylabel("Lines of code (stacked)", fontsize=12)
    ax.set_xlabel("Date (2026)", fontsize=12)
    ax.grid(True, axis="y", color="#000000", alpha=0.06, linewidth=0.8)
    ax.set_axisbelow(True)
    for side in ("top", "right"):
        ax.spines[side].set_visible(False)
    ax.margins(x=0, y=0)
    ymax = latest * 1.12
    ax.set_ylim(0, ymax)

    for day, name in release_tags():
        x = mdates.date2num(day)
        ax.axvline(x, color="#37474f", linewidth=0.7, linestyle=(0, (4, 3)), alpha=0.4)
        ax.annotate(
            name, xy=(x, ymax), xytext=(0, 4), textcoords="offset points",
            rotation=90, ha="center", va="bottom", fontsize=8,
            color="#37474f", annotation_clip=False,
        )

    handles, labels = ax.get_legend_handles_labels()
    if view_name == "modules":
        labels = [
            f"{category}  ({100 * series[category][-1] / latest:.0f}%)"
            for category in categories
        ]
    ax.legend(
        handles[::-1], labels[::-1], loc="upper left", fontsize=10,
        frameon=False, title=spec["legend"], bbox_to_anchor=(1.005, 1.0),
        borderaxespad=0,
    )
    ax.xaxis.set_major_locator(mdates.WeekdayLocator(byweekday=mdates.MO))
    ax.xaxis.set_major_formatter(mdates.DateFormatter("%b %d"))
    ax.xaxis.set_minor_locator(mdates.DayLocator())
    fig.subplots_adjust(left=0.07, right=0.84, top=0.9, bottom=0.1)
    fig.savefig(spec["output"])
    plt.close(fig)
    print(f"saved {spec['output']}")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--view", choices=["all", *VIEWS], default="all")
    args = parser.parse_args()
    configure_plot_style()
    commits = day_commits()
    selected = VIEWS if args.view == "all" else [args.view]
    for view_name in selected:
        render(view_name, commits)


if __name__ == "__main__":
    main()
