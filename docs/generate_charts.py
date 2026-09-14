"""Generate the charts used in README.md.

Every chart except the book walk and the benchmark chart is computed from the
real library through the Python bindings, so the figures stay in step with the
code. The book walk is arithmetic over a fixed, illustrative ladder of asks, and
the benchmark chart plots numbers measured with `cargo bench -p orderbook`.

Run from the repository root after installing the bindings:

    pip install -r python/requirements.txt
    pip install ./python matplotlib
    python docs/generate_charts.py            # writes docs/images/*.svg
    python docs/generate_charts.py previews/  # also writes PNG previews there
"""

import sys
from pathlib import Path

import matplotlib

matplotlib.use("Agg")
import matplotlib.pyplot as plt  # noqa: E402
import numpy as np  # noqa: E402

import algo_exec_py as ae  # noqa: E402

OUT = Path(__file__).resolve().parent / "images"
PREVIEW = Path(sys.argv[1]) if len(sys.argv) > 1 else None

SECOND = 1_000_000_000
MINUTE = 60 * SECOND
HOUR = 60 * MINUTE
DAY = 24 * HOUR

# Almgren-Chriss parameters used throughout the README.
SIGMA, ETA, GAMMA = 0.02, 0.5, 1e-6

INK = "#1f2328"
MUTED = "#57606a"
GRID = "#d8dee4"
BLUE = "#0969da"
ORANGE = "#bc4c00"
GREEN = "#1a7f37"
PURPLE = "#8250df"
RED = "#cf222e"
TEAL = "#1b7c83"

plt.rcParams.update(
    {
        "figure.facecolor": "white",
        "axes.facecolor": "white",
        "savefig.facecolor": "white",
        "axes.edgecolor": GRID,
        "axes.labelcolor": INK,
        "axes.titlecolor": INK,
        "axes.titleweight": "semibold",
        "axes.titlesize": 11.5,
        "axes.titlelocation": "left",
        "axes.labelsize": 10,
        "xtick.color": MUTED,
        "ytick.color": MUTED,
        "xtick.labelsize": 9,
        "ytick.labelsize": 9,
        "axes.grid": True,
        "grid.color": GRID,
        "grid.linewidth": 0.6,
        "axes.axisbelow": True,
        "axes.spines.top": False,
        "axes.spines.right": False,
        "font.family": "DejaVu Sans",
        "legend.frameon": False,
        "legend.fontsize": 9,
        "svg.hashsalt": "algo-execution-engine",
    }
)


def save(fig, name):
    OUT.mkdir(parents=True, exist_ok=True)
    fig.tight_layout()
    fig.savefig(OUT / f"{name}.svg", metadata={"Date": None})
    if PREVIEW is not None:
        PREVIEW.mkdir(parents=True, exist_ok=True)
        fig.savefig(PREVIEW / f"{name}.png", dpi=110)
    plt.close(fig)
    print(f"wrote {name}")


def quantities(schedule):
    return [qty for _, qty in schedule]


def label_bars(ax, bars, values, fmt="{:,}"):
    for bar, value in zip(bars, values):
        ax.annotate(
            fmt.format(value),
            (bar.get_x() + bar.get_width() / 2, bar.get_height()),
            xytext=(0, 3),
            textcoords="offset points",
            ha="center",
            va="bottom",
            fontsize=8.5,
            color=INK,
        )


def book_walk():
    """Price paid by one large market order against a ladder of asks."""
    ladder = [(100.50, 20_000), (101.00, 30_000), (101.50, 40_000), (102.00, 50_000)]
    order = 100_000

    fig, (depth, walk) = plt.subplots(1, 2, figsize=(10.5, 3.9), gridspec_kw={"width_ratios": [1, 1.5]})

    prices = [f"{p:.2f}" for p, _ in ladder]
    sizes = [q for _, q in ladder]
    bars = depth.barh(prices, sizes, color=[BLUE, "#6ea8fe", "#9ec5fe", "#cfe2ff"], height=0.6)
    depth.set_title("Asks resting in the book")
    depth.set_xlabel("quantity available")
    depth.set_ylabel("price")
    depth.grid(axis="y", visible=False)
    for bar, size in zip(bars, sizes):
        depth.annotate(f"{size:,}", (bar.get_width(), bar.get_y() + bar.get_height() / 2),
                       xytext=(4, 0), textcoords="offset points", va="center", fontsize=8.5, color=INK)
    depth.set_xlim(0, 62_000)
    depth.xaxis.set_major_formatter(matplotlib.ticker.StrMethodFormatter("{x:,.0f}"))

    filled, cost = 0, 0.0
    xs, marginal, average = [0], [ladder[0][0]], [ladder[0][0]]
    for price, size in ladder:
        take = min(size, order - filled)
        if take <= 0:
            break
        for step in np.linspace(0, take, 40)[1:]:
            xs.append(filled + step)
            marginal.append(price)
            average.append((cost + price * step) / (filled + step))
        filled += take
        cost += price * take

    walk.step(xs, marginal, where="pre", color=MUTED, linewidth=1.4, label="price of the next unit")
    walk.plot(xs, average, color=RED, linewidth=2.2, label="average price paid so far")
    walk.axhline(ladder[0][0], color=GREEN, linestyle="--", linewidth=1.2)
    walk.axvline(16_667, color=GREEN, linestyle=":", linewidth=1.2)
    walk.annotate("a 16,667-unit TWAP child\nstays at the best ask", (16_667, 100.52),
                  xytext=(21_000, 101.32), fontsize=8.5, color=GREEN,
                  arrowprops={"arrowstyle": "->", "color": GREEN, "linewidth": 0.9})
    walk.annotate(f"one market order for 100,000\naverages {cost / filled:.2f}", (100_000, cost / filled),
                  xytext=(52_000, 101.62), fontsize=8.5, color=RED,
                  arrowprops={"arrowstyle": "->", "color": RED, "linewidth": 0.9})
    walk.set_title("Walking the book with a single market order")
    walk.set_xlabel("quantity filled")
    walk.set_ylabel("price")
    walk.set_ylim(100.35, 102.15)
    walk.xaxis.set_major_formatter(matplotlib.ticker.StrMethodFormatter("{x:,.0f}"))
    walk.legend(loc="upper left")
    save(fig, "book_walk")


def twap_rounding():
    """Cumulative rounding against the naive split, 103 units over 5 slices."""
    total, n = 103, 5
    exact = quantities(ae.twap_schedule(0, HOUR, total, n))
    base, remainder = divmod(total, n)
    naive = [base + (1 if k < remainder else 0) for k in range(n)]
    ideal = np.array([total * (k + 1) / n for k in range(n)])

    fig, (left, right) = plt.subplots(1, 2, figsize=(10.5, 3.7))
    x = np.arange(1, n + 1)
    width = 0.38
    naive_bars = left.bar(x - width / 2, naive, width, color=ORANGE, label="naive split")
    exact_bars = left.bar(x + width / 2, exact, width, color=BLUE, label="cumulative rounding")
    label_bars(left, naive_bars, naive)
    label_bars(left, exact_bars, exact)
    left.set_ylim(0, 31)
    left.set_xticks(x)
    left.set_xlabel("slice")
    left.set_ylabel("child quantity")
    left.set_title("103 units over 5 slices")
    left.legend(loc="upper center", ncols=2)
    left.grid(axis="x", visible=False)

    right.axhspan(-0.5, 0.5, color="#ddf4e4", zorder=0)
    right.plot(x, np.cumsum(naive) - ideal, marker="o", color=ORANGE, linewidth=2, label="naive")
    right.plot(x, np.cumsum(exact) - ideal, marker="o", color=BLUE, linewidth=2, label="cumulative rounding")
    right.axhline(0, color=MUTED, linewidth=0.8)
    right.text(1.08, -0.46, "within half a unit", color=GREEN, fontsize=8.5, va="bottom")
    right.set_xticks(x)
    right.set_xlabel("after slice")
    right.set_ylabel("units done minus ideal")
    right.set_title("Cumulative error against the ideal straight line")
    right.set_ylim(-0.8, 1.45)
    right.legend(loc="upper right")
    save(fig, "twap_rounding")


def four_schedules():
    """The same 100 units over one hour, scheduled four ways."""
    total, n = 100, 6
    starts = [k * 10 for k in range(n)]

    twap = ae.twap_schedule(0, HOUR, total, n)
    profile = [4.0, 2.0, 1.0, 1.0, 2.0, 4.0]
    vwap = ae.vwap_schedule(0, HOUR, total, profile)
    prints = [300, 150, 80, 90, 160, 320]
    pov = ae.pov_schedule(0, HOUR, total, 1_000, [((5 + 10 * k) * MINUTE, v) for k, v in enumerate(prints)])
    shortfall = ae.almgren_chriss_schedule(
        start_ns=0, end_ns=HOUR, total_qty=total, num_slices=n, risk_aversion=1e-3,
        volatility=SIGMA, temporary_impact=ETA, permanent_impact=GAMMA,
    )

    panels = [
        ("TWAP: equal slices", twap, BLUE, "even split, remainder spread out"),
        ("VWAP: follow the volume profile", vwap, TEAL, "profile weights 4, 2, 1, 1, 2, 4"),
        ("POV: 10% of each print", pov["children"], GREEN, "prints of 300, 150, 80, 90, 160, 320"),
        ("Implementation shortfall: λ = 0.001", shortfall["children"], PURPLE, "front-loaded to cut risk"),
    ]
    fig, axes = plt.subplots(2, 2, figsize=(10.5, 6.3), sharey=True)
    for ax, (title, schedule, color, note) in zip(axes.flat, panels):
        times = [t / MINUTE for t, _ in schedule]
        qty = quantities(schedule)
        bars = ax.bar(times, qty, width=7.5, color=color, align="edge" if "POV" not in title else "center")
        label_bars(ax, bars, qty)
        ax.set_title(title)
        ax.text(0.99, 0.95, note, transform=ax.transAxes, ha="right", va="top", fontsize=8.5, color=MUTED)
        ax.set_xlim(-2, 62)
        ax.set_xticks(starts + [60])
        ax.set_ylim(0, 50)
        ax.grid(axis="x", visible=False)
    for ax in axes[1]:
        ax.set_xlabel("minutes into the window")
    for ax in axes[:, 0]:
        ax.set_ylabel("child quantity")
    save(fig, "four_schedules")


def almgren_chriss_trajectories():
    """Holdings over time for several risk aversions, and how kappa scales."""
    total, n = 100_000, 60
    lambdas = [0.0, 1e-4, 1e-3, 1e-2, 1e-1]
    colors = ["#8c959f", "#6ea8fe", BLUE, PURPLE, RED]

    fig, (left, right) = plt.subplots(1, 2, figsize=(10.5, 4.0))
    minutes = np.arange(n + 1)
    for lam, color in zip(lambdas, colors):
        result = ae.almgren_chriss_schedule(
            start_ns=0, end_ns=HOUR, total_qty=total, num_slices=n, risk_aversion=lam,
            volatility=SIGMA, temporary_impact=ETA, permanent_impact=GAMMA,
        )
        holdings = np.array(result["holdings"]) / total
        kappa = result["kappa"]
        label = "λ = 0 (TWAP)" if lam == 0 else f"λ = {lam:g},  1/κ = {1 / kappa / 60:.0f} min"
        left.plot(minutes, holdings, color=color, linewidth=2.2, label=label)
    left.set_title("Optimal holdings: more risk aversion trades sooner")
    left.set_xlabel("minutes into the window")
    left.set_ylabel("fraction still to trade")
    left.set_xlim(0, 60)
    left.set_ylim(0, 1.02)
    left.legend(loc="upper right")

    tau = HOUR / SECOND / n
    eta_tilde = ETA - 0.5 * GAMMA * tau
    grid = np.logspace(-8, 2, 80)
    kappas = [
        ae.almgren_chriss_schedule(
            start_ns=0, end_ns=HOUR, total_qty=total, num_slices=n, risk_aversion=float(lam),
            volatility=SIGMA, temporary_impact=ETA, permanent_impact=GAMMA,
        )["kappa"]
        for lam in grid
    ]
    right.loglog(grid, kappas, color=BLUE, linewidth=2.2, label="κ solved on the discrete grid")
    right.loglog(grid, np.sqrt(grid * SIGMA**2 / eta_tilde), color=ORANGE, linestyle="--",
                 linewidth=1.6, label="fine-slice approximation  √(λσ²/η̃)")
    right.set_title("κ follows √λ until the slice length binds")
    right.set_xlabel("risk aversion λ")
    right.set_ylabel("κ  (1 / second)")
    right.legend(loc="upper left")
    save(fig, "almgren_chriss_trajectories")


def efficient_frontier():
    """Expected cost against the standard deviation of cost as lambda varies."""
    total, n = 100_000, 60
    common = dict(start_ns=0, end_ns=HOUR, total_qty=total, num_slices=n,
                  volatility=SIGMA, temporary_impact=ETA, permanent_impact=GAMMA)
    lambdas = np.concatenate([[0.0], np.logspace(-6, 0, 90)])
    points = [ae.almgren_chriss_schedule(risk_aversion=float(lam), **common) for lam in lambdas]
    risk = np.sqrt([p["cost_variance"] for p in points])
    cost = np.array([p["expected_cost"] for p in points])

    fig, ax = plt.subplots(figsize=(8.2, 4.4))
    ax.plot(risk, cost, color=BLUE, linewidth=2.4, zorder=2)
    marks = [(0.0, "TWAP, λ = 0: lowest expected cost,\nmost exposure to price moves"), (1e-3, "λ = 0.001"),
             (1e-2, "λ = 0.01"), (1e-1, "λ = 0.1: little risk,\nhigh impact cost")]
    offsets = [(-8, 58), (-10, -20), (10, 4), (12, -2)]
    for (lam, text), offset in zip(marks, offsets):
        p = ae.almgren_chriss_schedule(risk_aversion=lam, **common)
        x, y = np.sqrt(p["cost_variance"]), p["expected_cost"]
        ax.scatter([x], [y], color=RED if lam == 0 else PURPLE, s=36, zorder=3)
        ax.annotate(text, (x, y), xytext=offset, textcoords="offset points", fontsize=8.5, color=INK,
                    ha="left" if offset[0] > 0 else "right",
                    arrowprops={"arrowstyle": "->", "color": MUTED, "linewidth": 0.8} if lam == 0 else None)
    ax.set_yscale("log")
    ticks = [1.5e6, 3e6, 1e7, 3e7]
    ax.set_yticks(ticks, [f"{t / 1e6:g}M" for t in ticks])
    ax.yaxis.set_minor_formatter(matplotlib.ticker.NullFormatter())
    ax.set_title("The efficient frontier: every point is optimal for some λ")
    ax.set_xlabel("standard deviation of cost  √V  (currency units)")
    ax.set_ylabel("expected cost  E  (currency units, log scale)")
    ax.xaxis.set_major_formatter(matplotlib.ticker.StrMethodFormatter("{x:,.0f}"))
    save(fig, "efficient_frontier")


def pov_cap():
    """The participation cap and completion, and what a shortfall looks like."""
    rng = np.random.default_rng(7)
    minutes = np.arange(60)
    shape = 1.0 + 1.5 * ((minutes - 29.5) / 29.5) ** 2
    volumes = np.rint(1_000 * shape * rng.lognormal(0.0, 0.35, size=60)).astype(int)
    stream = [(int((m + 0.5) * MINUTE), int(v)) for m, v in zip(minutes, volumes)]
    cap = np.floor(np.cumsum(volumes) * 1_000 / 10_000)
    times = minutes + 0.5

    fig, axes = plt.subplots(1, 2, figsize=(10.5, 3.9), sharey=True)
    for ax, total, title in [
        (axes[0], 5_000, "Enough volume: the order completes and stops"),
        (axes[1], 12_000, "Too little volume: the rest is reported as shortfall"),
    ]:
        result = ae.pov_schedule(0, HOUR, total, 1_000, stream)
        child_times = [t / MINUTE for t, _ in result["children"]]
        done = np.cumsum(quantities(result["children"]))
        ax.plot(times, cap, color=MUTED, linewidth=1.4, linestyle="--", label="cap: 10% of cumulative volume")
        ax.step([0] + child_times + [60], [0] + list(done) + [done[-1]], where="post", color=GREEN,
                linewidth=2.2, label="scheduled so far")
        ax.axhline(total, color=RED, linewidth=1.1, linestyle=":", label=f"order size {total:,}")
        if result["shortfall_qty"]:
            ax.annotate(f"shortfall {result['shortfall_qty']:,}", (60, done[-1]), xytext=(-8, 26),
                        textcoords="offset points", ha="right", fontsize=8.5, color=RED,
                        arrowprops={"arrowstyle": "->", "color": RED, "linewidth": 0.9})
        ax.set_title(title)
        ax.set_xlabel("minutes into the window")
        ax.set_xlim(0, 60)
        ax.legend(loc="lower right")
        ax.yaxis.set_major_formatter(matplotlib.ticker.StrMethodFormatter("{x:,.0f}"))
    axes[0].set_ylabel("units")
    save(fig, "pov_cap")


def vwap_profile():
    """A profile built from historical bars, and the schedule that follows it."""
    open_ns = 9 * HOUR + 30 * MINUTE
    session = 6 * HOUR + 30 * MINUTE
    bars = [(day * DAY + open_ns + m * MINUTE, 1_000 + 40 * abs(195 - m))
            for day in range(5) for m in range(0, 390, 30)]
    n = 13
    total = 10_000
    profile = ae.volume_profile_from_bars(bars, open_ns, session, n)
    schedule = ae.vwap_schedule(0, session, total, profile)

    fig, ax = plt.subplots(figsize=(10.5, 3.8))
    x = np.arange(n)
    qty = quantities(schedule)
    bars_drawn = ax.bar(x, qty, color=TEAL, width=0.7, label="child quantity")
    for bar, value in zip(bars_drawn, qty):
        ax.annotate(f"{value:,}", (bar.get_x() + bar.get_width() / 2, bar.get_height()), xytext=(0, -4),
                    textcoords="offset points", ha="center", va="top", fontsize=8, color="white")
    ax.plot(x, np.array(profile) * total, color=ORANGE, marker="o", markersize=4, linewidth=1.4,
            label="profile share × order size, before rounding")
    labels = [f"{9 + (30 + 30 * k) // 60:02d}:{(30 + 30 * k) % 60:02d}" for k in range(n)]
    ax.set_xticks(x, labels)
    ax.set_xlabel("slice start (time of day)")
    ax.set_ylabel("units")
    ax.set_ylim(0, max(qty) * 1.22)
    ax.set_title("VWAP over a trading session: 10,000 units following a U-shaped volume profile")
    ax.legend(loc="upper center", ncols=2)
    ax.grid(axis="x", visible=False)
    save(fig, "vwap_profile")


def randomized_twap():
    """Where randomized release times land, and their distribution inside a slice."""
    fig, (left, right) = plt.subplots(1, 2, figsize=(10.5, 3.6), gridspec_kw={"width_ratios": [1.5, 1]})
    n = 10
    rows = [("plain TWAP", ae.twap_schedule(0, 10 * SECOND, 100, n))]
    rows += [(f"seed {seed}", ae.twap_schedule_randomized(0, 10 * SECOND, 100, n, seed)) for seed in (1, 2, 3)]
    for boundary in range(n + 1):
        left.axvline(boundary, color=GRID, linewidth=0.9, zorder=0)
    for row, (name, schedule) in enumerate(rows):
        times = [t / SECOND for t, _ in schedule]
        left.scatter(times, [row] * len(times), marker="|", s=380, linewidths=2.2,
                     color=MUTED if row == 0 else [BLUE, PURPLE, TEAL][row - 1])
    left.set_yticks(range(len(rows)), [name for name, _ in rows])
    left.invert_yaxis()
    left.set_xlim(-0.2, 10.2)
    left.set_xticks(range(n + 1))
    left.set_xlabel("seconds (grey lines are slice boundaries)")
    left.set_title("One release per slice, somewhere inside it")
    left.grid(visible=False)

    offsets = []
    for seed in range(200):
        for k, (t, _) in enumerate(ae.twap_schedule_randomized(0, 100 * SECOND, 100, 100, seed)):
            offsets.append((t - k * SECOND) / SECOND)
    right.hist(offsets, bins=20, range=(0, 1), density=True, color="#9ec5fe", edgecolor=BLUE, linewidth=0.8)
    right.axhline(1.0, color=RED, linestyle="--", linewidth=1.2, label="uniform")
    right.set_xlim(0, 1)
    right.set_ylim(0, 1.35)
    right.set_xlabel("position inside the slice")
    right.set_ylabel("density")
    right.set_title(f"Offsets of {len(offsets):,} draws")
    right.legend(loc="upper right")
    save(fig, "randomized_twap")


def benchmarks():
    """Criterion results, measured with `cargo bench -p orderbook`."""
    rows = [
        ("read best bid and best ask (1,000 levels)", 4.8),
        ("insert + cancel a passive order (10 levels)", 85.4),
        ("insert + cancel a passive order (1,000 levels)", 96.1),
        ("IOC takes the best ask + refill (10 levels)", 131.9),
        ("IOC takes the best ask + refill (1,000 levels)", 152.9),
    ]
    fig, ax = plt.subplots(figsize=(9.2, 3.5))
    names = [name for name, _ in rows]
    values = [value for _, value in rows]
    bars = ax.barh(names, values, color=[GREEN, BLUE, BLUE, PURPLE, PURPLE], height=0.6)
    for bar, value in zip(bars, values):
        ax.annotate(f"{value:.1f} ns", (bar.get_width(), bar.get_y() + bar.get_height() / 2), xytext=(5, 0),
                    textcoords="offset points", va="center", fontsize=9, color=INK)
    ax.invert_yaxis()
    ax.set_xlim(0, 185)
    ax.set_xlabel("nanoseconds per operation (lower is better)")
    ax.set_title("Order book microbenchmarks (single thread)")
    ax.grid(axis="y", visible=False)
    ax.text(0.99, 0.95, "100,000 mixed operations in steady state:\n11.1 ms, about 9.0 million operations per second",
            transform=ax.transAxes, ha="right", va="top", fontsize=9, color=INK,
            bbox={"boxstyle": "round,pad=0.5", "facecolor": "#f6f8fa", "edgecolor": GRID})
    save(fig, "benchmarks")


if __name__ == "__main__":
    book_walk()
    twap_rounding()
    four_schedules()
    vwap_profile()
    pov_cap()
    randomized_twap()
    almgren_chriss_trajectories()
    efficient_frontier()
    benchmarks()
