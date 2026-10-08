# The committed benchmark baseline

These are criterion's own saved results for the twenty benchmarks in
`core/benches/`, taken with

```sh
cargo bench --all-features -- --profile ci --save-baseline main
```

and copied out of `core/target/criterion/<id>/main/`, which is where criterion
puts them. There is no other transformation: every file here is byte-for-byte what
criterion wrote, so refreshing the baseline is a copy and nothing else.

They are here so a comparison is possible later. `.github/workflows/bench.yml`
copies this tree back into `core/target/criterion/` before running the suite with
`--baseline main`, and `scripts/bench-compare.py` then reads the two
`estimates.json` files and applies the 15% rule.

## What is stored

| File | What it is | Size |
| --- | --- | ---: |
| `estimates.json` | the number the comparison uses: criterion's bootstrap estimates of mean, median, slope and MAD, with 95% confidence intervals, in nanoseconds | 18 KB total |
| `benchmark.json` | the group, the filter and the throughput unit, so criterion can render the comparison report | 4 KB total |
| `sample.json` | iteration and warm-up counts for the run | 4 KB total |
| `tukey.json` | the outlier fences for the same run | 1 KB total |

`raw.csv` is **not** stored, because it only exists when criterion is built with
its `csv_output` feature, and this suite is not — `plotters` and `rayon` are both
excluded from the dependency, so there is nothing to draw and nothing to sample
into a CSV.

## Refreshing it

```sh
cd core
rm -rf target/criterion
cargo bench --all-features -- --profile ci --save-baseline main
rm -rf benches/baseline
mkdir -p benches/baseline
for d in $(find target/criterion -type d -name main); do
  id=${d#target/criterion/}; id=${id%/main}
  mkdir -p "benches/baseline/$id"
  cp "$d"/*.json "benches/baseline/$id/"
done
```

Two things to do that are not mechanical:

1. **Say in the commit message why it moved.** A baseline that changes with the
   commit message next to it is the only way a reader can tell a legitimate
   re-measurement from a number that drifted because the runner was busy.
2. **Update the tables in `docs/BENCHMARKS.md`** if any of them came from this
   run. They are checked against the committed numbers by hand, not by a script,
   and a stale table that nobody notices is worse than no table.

## The caveat that matters more than the numbers

**A baseline is a measurement of one machine.** These were taken on the runner
described in `docs/BENCHMARKS.md` — an x86-64 Xeon with AVX2, four visible
threads, shared with whatever else the CI box is doing. Comparing them against a
run on different hardware does not measure a regression; it measures the
hardware. `bench.yml` records the CPU model of every run in the job summary and
prints a warning when it does not match the one recorded here in `machine.json`,
but it cannot stop the comparison from being meaningless.

That is why the gate's threshold is 15% and not 5% — see the workflow for the
full argument — and why a regression that lands on a machine change is worth
confirming by hand before anybody goes looking for the commit that caused it.
