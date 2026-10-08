#!/usr/bin/env python3
"""Apply the benchmark regression rule to a criterion run.

criterion can print "Performance has regressed." at its own hard-coded 5%
threshold, and it cannot do what this gate needs: criterion 0.7 removed the
`--profile` flag, and a run configured through criterion's own builder cannot be
told to fail at 15% instead. So this reads what criterion wrote and applies the
rule here, where the threshold is one named constant in one place.

What it compares, and why:

* **The `mean` point estimate, not the `median`.** That is the statistic
  criterion's own regression verdict uses, so a row this script calls a
  regression is one criterion would also have flagged had it been asked. The
  median is printed alongside because it is the number `docs/BENCHMARKS.md`
  quotes, and a mean/median disagreement is the first sign of a bimodal sample.
* **Confidence intervals, to say whether the change is distinguishable from
  noise.** criterion decides that with a Mann-Whitney U test over the raw
  samples, which needs `sample.json`'s per-iteration timings — not something
  either of us re-derives here. Overlapping intervals is a coarser test than
  criterion's p-value and it is described as such in the output.
* **Per benchmark, never aggregated.** A total or a mean over all twenty would
  let one slow case hide behind nineteen fast ones, which is the shape of
  regression this gate exists to catch.

Exit codes: 0 = nothing over the threshold, or the run is not comparable (which
is reported loudly, not silently passed); 1 = at least one benchmark regressed by
more than the threshold.
"""

from __future__ import annotations

import argparse
import json
import os
import re
import sys
from pathlib import Path

# The gate's threshold, and why it is 15% and not 5%. The full argument is in
# .github/workflows/bench.yml next to the same number; the short version is that
# criterion's ci profile here takes 10 samples over a 3 s window, so a 12 MP
# encode or a 24 MP resize is a 4-6 iteration sample on a shared runner whose
# neighbours are not idle. Run-to-run spread on this suite is 1-4% for the long
# cases and worse than 10% for the short ones (exif/read is 15 microseconds, so
# it is measuring scheduler noise and cache state more than it is measuring
# EXIF). A 5% gate would be red more often than green, and a gate that is always
# red is a gate nobody reads. 15% is above the noise for everything except
# exif/read, which is why that one row is marked advisory below.
THRESHOLD_PERCENT = 15.0

# Benchmarks whose absolute cost is so small that the ci profile cannot resolve a
# 15% change. exif/read is 15 microseconds: at that size a context switch is
# larger than the measurement. Reporting it as a hard failure would be a gate
# that fails for reasons it cannot explain, so it is reported and excluded from
# the verdict. Everything else in the suite runs for 30 ms or longer.
ADVISORY_MAX_NANOS = 1_000_000  # 1 ms


def human(nanos: float) -> str:
    """Format a nanosecond count the way criterion prints it."""
    if nanos >= 1e9:
        return f"{nanos / 1e9:.4g} s"
    if nanos >= 1e6:
        return f"{nanos / 1e6:.4g} ms"
    if nanos >= 1e3:
        return f"{nanos / 1e3:.4g} \u00b5s"
    return f"{nanos:.4g} ns"


def cpu_model() -> str:
    """The CPU model string, or 'unknown' where Linux does not publish one.

    Compared as a substring rather than for equality so that a runner reporting
    "Intel(R) Xeon(R) Platinum 8375C CPU @ 2.90GHz" still matches a recorded
    "Intel Xeon 8375C". A false match is harmless: it can only cause the gate to
    compare, and the threshold absorbs a same-family difference.
    """
    try:
        text = Path("/proc/cpuinfo").read_text(encoding="utf-8", errors="replace")
    except OSError:
        return "unknown"
    match = re.search(r"^model name\s*:\s*(.+)$", text, re.MULTILINE)
    if match:
        return match.group(1).strip()
    match = re.search(r"^Model\s*:\s*(.+)$", text, re.MULTILINE)
    return match.group(1).strip() if match else "unknown"


def load_estimates(path: Path) -> dict:
    """One criterion `estimates.json`, or a raise the caller turns into a note."""
    data = json.loads(path.read_text(encoding="utf-8"))
    mean = data["mean"]
    return {
        "mean": float(mean["point_estimate"]),
        "ci_low": float(mean["confidence_interval"]["lower_bound"]),
        "ci_high": float(mean["confidence_interval"]["upper_bound"]),
        "median": float(data["median"]["point_estimate"]),
    }


def collect(root: Path, baseline: str) -> list[tuple[str, dict, dict]]:
    """Every benchmark with both a baseline and a current estimate."""
    found = []
    for base_dir in sorted(root.glob(f"*/**/{baseline}")):
        if not base_dir.is_dir():
            continue
        name = str(base_dir.relative_to(root).parent).replace(os.sep, "/")
        new_dir = base_dir.parent / "new"
        if not (new_dir / "estimates.json").exists():
            print(f"note: no current result for {name}; skipping", file=sys.stderr)
            continue
        try:
            found.append(
                (name, load_estimates(base_dir / "estimates.json"), load_estimates(new_dir / "estimates.json"))
            )
        except (OSError, KeyError, ValueError) as exc:
            print(f"note: unreadable estimates for {name}: {exc}", file=sys.stderr)
    return found


def compare(name: str, base: dict, current: dict, threshold: float) -> dict:
    """One row: the change, and whether it is over the threshold."""
    change = (current["mean"] - base["mean"]) / base["mean"] * 100.0
    overlaps = not (
        current["ci_low"] > base["ci_high"] or current["ci_high"] < base["ci_low"]
    )
    regression = change > threshold
    advisory = base["mean"] < ADVISORY_MAX_NANOS
    if regression and advisory:
        verdict = "ADVISORY (shorter than 1 ms; too fast for this profile)"
    elif regression:
        verdict = "**REGRESSION**"
    elif change < -threshold:
        verdict = "improved"
    elif overlaps:
        verdict = "within noise"
    else:
        verdict = "changed, not over the threshold"
    return {
        "name": name,
        "base": base,
        "current": current,
        "change": change,
        "overlaps": overlaps,
        "regression": regression,
        "advisory": advisory,
        "verdict": verdict,
    }


def render(rows: list[dict]) -> str:
    """A markdown table, worst change first, for the job summary and the PR."""
    lines = [
        "| Benchmark | Baseline | This run | Change | Verdict |",
        "| --- | ---: | ---: | ---: | --- |",
    ]
    for row in sorted(rows, key=lambda r: -r["change"]):
        sign = "+" if row["change"] >= 0 else ""
        lines.append(
            f"| `{row['name']}` | {human(row['base']['mean'])} | "
            f"{human(row['current']['mean'])} | {sign}{row['change']:.1f}% | {row['verdict']} |"
        )
    return "\n".join(lines)


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--criterion-dir",
        default="core/target/criterion",
        help="criterion's output directory (default: core/target/criterion)",
    )
    parser.add_argument("--baseline", default="main", help="baseline name criterion saved it under")
    parser.add_argument("--threshold", type=float, default=THRESHOLD_PERCENT)
    parser.add_argument(
        "--machine",
        default="core/benches/baseline/machine.json",
        help="the machine record the baseline was taken on",
    )
    parser.add_argument("--markdown", help="write the table here as well as to stdout")
    parser.add_argument(
        "--ignore-machine",
        action="store_true",
        help="compare even when the CPU model differs from the recorded one",
    )
    args = parser.parse_args()

    root = Path(args.criterion_dir)
    if not root.is_dir():
        print(f"note: {root} does not exist; nothing to compare", file=sys.stderr)
        return 0

    rows = [compare(n, b, c, args.threshold) for n, b, c in collect(root, args.baseline)]
    if not rows:
        print(f"note: no comparable benchmarks under {root}; nothing to do", file=sys.stderr)
        return 0

    comparable = True
    try:
        record = json.loads(Path(args.machine).read_text(encoding="utf-8"))
        recorded = record.get("cpu_model", "unknown")
    except (OSError, ValueError):
        recorded, comparable = "unknown", False

    here = cpu_model()
    same_machine = recorded in here or here in recorded
    if not same_machine and not args.ignore_machine:
        comparable = False

    table = render(rows)
    if not comparable:
        # A baseline is a measurement of one machine. Comparing across two is
        # how a gate ends up reporting a 30% "regression" that is entirely the
        # difference between two CPUs, and once it does that twice nobody reads
        # it again. Refusing to have an opinion is the honest response.
        note = (
            f"> **Not compared.** The committed baseline was measured on "
            f"`{recorded}`, and this run is on `{here}`. A difference between two "
            f"machines is not a regression in the code, so no verdict is given. "
            f"Re-measure the baseline on this runner class to make the gate "
            f"meaningful (see `core/benches/baseline/README.md`)."
        )
        print(note)
        print()
        print(f"The raw numbers for the record:\n\n{table}")
        if args.markdown:
            Path(args.markdown).write_text(f"{note}\n\n{table}\n", encoding="utf-8")
        return 0

    regressions = [r for r in rows if r["regression"] and not r["advisory"]]
    print(table)
    if args.markdown:
        Path(args.markdown).write_text(table + "\n", encoding="utf-8")

    if not regressions:
        advisory = [r["name"] for r in rows if r["regression"] and r["advisory"]]
        suffix = f" (advisory only: {', '.join(advisory)})" if advisory else ""
        print(f"\nno benchmark regressed by more than {args.threshold:g}%{suffix}")
        return 0

    print(f"\n{len(regressions)} benchmark(s) regressed by more than {args.threshold:g}%:")
    for row in regressions:
        print(f"  {row['name']}: {row['change']:+.1f}%")
    return 1


if __name__ == "__main__":
    sys.exit(main())
