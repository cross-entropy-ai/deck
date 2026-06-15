#!/usr/bin/env python3
"""Publication-quality daily candlestick (K-line) of src/**/*.rs line count.

Top panel  : daily K-line of total Rust LOC under src/.
               open  = day start (prev close), close = day end,
               high/low = intraday extremes. Green = net growth, red = shrink.
Bottom panel: daily churn (added stacked over deleted lines).
Release tags (minor versions + latest) are annotated on the price panel.
"""

import argparse
import re
import subprocess
from collections import OrderedDict
from datetime import datetime

import matplotlib.pyplot as plt
import matplotlib.dates as mdates
from matplotlib.patches import Rectangle, Patch

UP, DOWN = "#26a69a", "#ef5350"   # teal / red, classic candlestick palette
TAG_C = "#37474f"                 # slate for tag markers
W = 0.62                          # candle / bar width in days

# What to count. Override on the CLI; default = Rust source under src/.
PATHSPEC = ["src/*.rs"]           # git pathspec(s); empty = whole repo
SCOPE = "Rust under src/"         # human label used in axis/title


def run(args):
    return subprocess.run(args, capture_output=True, text=True, check=True).stdout


def commits():
    """[(datetime, sha)] oldest -> newest."""
    out = run(["git", "log", "--reverse", "--format=%H %cI"])
    rows = []
    for line in out.splitlines():
        sha, iso = line.split(" ", 1)
        rows.append((datetime.fromisoformat(iso), sha))
    return rows


def rs_lines(sha):
    """Total lines across the configured pathspec at the given commit.

    `git grep -I` skips binary files, so the whole-repo case counts only
    text lines.
    """
    res = subprocess.run(
        ["git", "grep", "-I", "-c", "", sha, "--"] + PATHSPEC,
        capture_output=True, text=True,
    )
    return sum(int(l.rsplit(":", 1)[1]) for l in res.stdout.splitlines())


def daily_ohlc(rows):
    """Group commit line counts by calendar day into OHLC candles."""
    by_day = OrderedDict()
    for when, sha in rows:
        by_day.setdefault(when.date(), []).append(rs_lines(sha))
    candles, prev_close = [], None
    for day, locs in by_day.items():
        open_ = prev_close if prev_close is not None else locs[0]
        candles.append((day, open_, max([open_] + locs),
                        min([open_] + locs), locs[-1]))
        prev_close = locs[-1]
    return candles


def daily_churn():
    """date -> (added, deleted) lines touching the configured pathspec."""
    out = run(["git", "log", "--reverse", "--numstat", "--format=__%cI",
               "--"] + PATHSPEC)
    churn, day = OrderedDict(), None
    for line in out.splitlines():
        if line.startswith("__"):
            day = datetime.fromisoformat(line[2:]).date()
            continue
        if not line.strip() or day is None:
            continue
        add, dele, _path = line.split("\t", 2)
        if add == "-":          # binary file; --numstat reports "-"
            continue
        b = churn.setdefault(day, [0, 0])
        b[0] += int(add)
        b[1] += int(dele)
    return churn


def release_tags(minor_only=True):
    """[(date, name)] for release tags worth annotating."""
    out = run(["git", "tag", "--sort=creatordate",
               "--format=%(refname:short) %(creatordate:iso-strict)"])
    tags = []
    for line in out.splitlines():
        name, iso = line.split(" ", 1)
        tags.append((datetime.fromisoformat(iso).date(), name))
    if not tags:
        return tags
    if minor_only:
        tags = [(d, n) for d, n in tags if re.fullmatch(r"v\d+\.\d+\.0", n)] \
            + [tags[-1]]                 # minor bumps + always the latest
    # collapse tags landing on the same day -> keep the last (highest) one
    per_day = OrderedDict()
    for d, n in tags:
        per_day[d] = n
    return list(per_day.items())


def main():
    global PATHSPEC, SCOPE
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument("--pathspec", nargs="*", default=PATHSPEC,
                    help="git pathspec(s) to count; pass nothing for whole repo")
    ap.add_argument("--scope", default=SCOPE,
                    help="human label for the counted scope")
    ap.add_argument("--out", default="scripts/loc_over_time.png",
                    help="output image path")
    args = ap.parse_args()
    PATHSPEC, SCOPE = args.pathspec, args.scope

    plt.rcParams.update({
        "font.family": "sans-serif",
        "font.sans-serif": ["Helvetica Neue", "Helvetica", "Arial", "DejaVu Sans"],
        "font.size": 11,
        "axes.linewidth": 0.8,
        "axes.edgecolor": "#444444",
        "svg.fonttype": "none",
    })

    rows = commits()
    candles = daily_ohlc(rows)
    churn = daily_churn()
    tags = release_tags()
    print(f"{len(rows)} commits over {len(candles)} days, "
          f"{candles[0][1]} -> {candles[-1][4]} lines; {len(tags)} tags")

    fig, (ax, axv) = plt.subplots(
        2, 1, figsize=(13, 7.5), sharex=True, dpi=200,
        gridspec_kw={"height_ratios": [3, 1], "hspace": 0.06})

    # ---- price panel ----
    for day, o, h, l, c in candles:
        x = mdates.date2num(day)
        color = UP if c >= o else DOWN
        ax.plot([x, x], [l, h], color=color, lw=1.0, zorder=2,
                solid_capstyle="round")
        lo, hi = sorted((o, c))
        ax.add_patch(Rectangle((x - W / 2, lo), W, (hi - lo) or 0.6,
                               facecolor=color, edgecolor=color,
                               lw=0.5, zorder=3))

    ax.set_ylabel(f"Lines of code — {SCOPE}", fontsize=12)
    ax.set_title(f"deck — daily evolution of {SCOPE}",
                 fontsize=14, fontweight="bold", pad=30, loc="left")
    ax.grid(True, axis="y", color="#000000", alpha=0.06, lw=0.8)
    ax.set_axisbelow(True)
    for s in ("top", "right"):
        ax.spines[s].set_visible(False)
    ax.margins(x=0.015)

    # ---- release tag annotations (vertical guides + top labels) ----
    ymax = max(h for _, _, h, _, _ in candles)
    ax.set_ylim(0, ymax * 1.08)
    for day, name in tags:
        x = mdates.date2num(day)
        ax.axvline(x, color=TAG_C, lw=0.7, ls=(0, (4, 3)), alpha=0.45, zorder=1)
        ax.annotate(name, xy=(x, ymax * 1.085),
                    xytext=(0, 4), textcoords="offset points",
                    rotation=90, ha="center", va="bottom",
                    fontsize=8, color=TAG_C, fontweight="medium",
                    annotation_clip=False)

    # ---- volume panel: churn (added + deleted lines per day) ----
    max_churn = max((a + d for a, d in churn.values()), default=1)
    for day, (add, dele) in churn.items():
        x = mdates.date2num(day)
        axv.bar(x, add, width=W, color=UP, zorder=3)
        axv.bar(x, dele, width=W, bottom=add, color=DOWN, zorder=3)
    axv.set_ylim(0, max_churn * 1.12)   # headroom so the tallest bar isn't clipped

    axv.set_ylabel("Daily churn\n(lines)", fontsize=12)
    axv.set_xlabel("Date (2026)", fontsize=12)
    axv.grid(True, axis="y", color="#000000", alpha=0.06, lw=0.8)
    axv.set_axisbelow(True)
    for s in ("top", "right"):
        axv.spines[s].set_visible(False)

    axv.legend(
        handles=[Patch(facecolor=UP, label="added lines"),
                 Patch(facecolor=DOWN, label="deleted lines")],
        loc="upper left", fontsize=9, frameon=False, ncol=2)
    # price-panel candles follow the usual convention: green up, red down
    ax.text(0.0, 1.0, "candles: green = net growth that day, red = net shrink",
            transform=ax.transAxes, fontsize=9, color="#666666",
            ha="left", va="bottom")

    axv.xaxis_date()
    axv.xaxis.set_major_locator(mdates.WeekdayLocator(byweekday=mdates.MO))
    axv.xaxis.set_major_formatter(mdates.DateFormatter("%b %d"))
    axv.xaxis.set_minor_locator(mdates.DayLocator())
    for lab in axv.get_xticklabels():
        lab.set_rotation(0)
        lab.set_ha("center")

    fig.subplots_adjust(left=0.07, right=0.985, top=0.9, bottom=0.08)

    fig.savefig(args.out)
    print(f"saved {args.out}")


if __name__ == "__main__":
    main()
