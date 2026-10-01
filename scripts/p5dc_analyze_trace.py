"""Summarize Debug native prediction CSVs; no simulated GPU results enter this report.

Usage: python scripts/p5dc_analyze_trace.py baseline.csv adaptive.csv [output.json]
Group geometry-refined generations by actual input sequence. Percentiles use
nearest rank; cached sub-millisecond commits still include real composition.
"""
from __future__ import annotations

import csv
import json
import math
import pathlib
import statistics
import sys

PHASES = (
    "cold_forward", "immediate_reverse", "warm_forward", "slow_forward",
    "medium_forward", "fast_forward", "reversal", "distant_jump_forward",
)


def percentile(values, fraction):
    return sorted(values)[max(0, math.ceil(len(values) * fraction) - 1)] if values else None


def summarize(path, mode):
    with pathlib.Path(path).open(newline="", encoding="utf-8") as stream:
        rows = [r for r in csv.DictReader(stream) if r["mode"] == mode
                and 0 <= int(r["benchmark_step"]) <= 192]
    done = [r for r in rows if r["event"] == "benchmark_done"]
    if len(done) != 1:
        raise ValueError(f"Expected one completed {mode} benchmark, found {len(done)}")
    requests, commits = {}, {}
    for row in rows:
        key = int(row["input"])
        if row["event"] == "request":
            requests.setdefault(key, row)
        elif row["event"] == "commit":
            commits[key] = row
    # Geometry completion for the initial Go-to-page prime is not a movement.
    requests = {k: r for k, r in requests.items()
                if not (r["benchmark_step"] == "0" and r["direction"] == "0")}
    if len(requests) != 192:
        raise ValueError(f"Expected 192 actual movements, found {len(requests)}")

    def phase_result(selected):
        observed = [(r, commits[k]) for k, r in selected if k in commits]
        input_ms = [(int(c["wall_us"]) - int(r["wall_us"])) / 1000 for r, c in observed]
        generation_ms = [int(c["latency_us"]) / 1000 for _, c in observed]
        required = sum(int(r["required"]) for _, r in selected)
        gpu = sum(int(r["entry_gpu"]) for _, r in selected)
        cpu = sum(int(r["entry_cpu"]) for _, r in selected)
        predicted = sum(int(r["entry_predictive_gpu"]) for _, r in selected)
        return {
            "requests": len(selected), "committed_inputs": len(observed),
            "coalesced_uncommitted_inputs": len(selected) - len(observed),
            "input_commit_median_ms": statistics.median(input_ms) if input_ms else None,
            "input_commit_p95_ms": percentile(input_ms, .95),
            "generation_commit_median_ms": statistics.median(generation_ms) if generation_ms else None,
            "generation_commit_p95_ms": percentile(generation_ms, .95),
            "entry_gpu_coverage_pct": 100 * gpu / required if required else 0,
            "entry_cpu_coverage_pct": 100 * cpu / required if required else 0,
            "entry_predictive_gpu_coverage_pct": 100 * predicted / required if required else 0,
            "fully_gpu_ready_requests": sum(int(r["required"]) > 0 and
                                            r["entry_gpu"] == r["required"] for _, r in selected),
            "arrival_holds": sum(int(r["entry_gpu"]) < int(r["required"]) for _, r in selected),
            "unknown_geometry_arrivals": sum(int(r["required"]) == 0 for _, r in selected),
            "longest_hold_ms": max((int(c["hold_us"]) / 1000 for _, c in observed), default=0),
        }

    all_selected = list(requests.items())
    first, end = all_selected[0][1], done[0]
    seconds = (int(end["wall_us"]) - int(first["wall_us"])) / 1e6
    pumps = [int(r["pump_us"]) / 1000 for r in rows if r["event"] == "pump"]
    result = {"source": str(path), "mode": mode, "seconds": seconds,
              "all": phase_result(all_selected), "phases": {}}
    for i, phase in enumerate(PHASES):
        result["phases"][phase] = phase_result([
            (k, r) for k, r in all_selected if int(r["benchmark_step"]) // 24 == i])
    result["established_cold_forward"] = phase_result([
        (k, r) for k, r in all_selected if 2 <= int(r["benchmark_step"]) < 24])
    result["continuous_forward"] = phase_result([
        (k, r) for k, r in all_selected if int(r["benchmark_step"]) // 24 in (0, 3, 4, 5)])
    result["upload_pump_p95_ms"] = percentile(pumps, .95)
    result["upload_pump_max_ms"] = max(pumps, default=0)
    result["partial"] = max(int(r["partial"]) for r in rows)
    result["cpu_peak_mib"] = max(int(r["cpu_peak"]) for r in rows) / (1 << 20)
    result["gpu_peak_mib"] = max(int(r["gpu_peak"]) for r in rows) / (1 << 20)
    for counter in ("renders", "uploads", "cpu_completed", "gpu_uploaded", "cpu_used",
                    "gpu_used", "cpu_wasted", "gpu_wasted", "invalidated"):
        result[counter] = int(end[counter]) - int(first[counter])
    result["renders_per_second"] = result["renders"] / seconds
    # Includes pending future tail; never-used at trace end is censored, not final waste.
    for lane, prepared in (("cpu", "cpu_completed"), ("gpu", "gpu_uploaded")):
        total = result[prepared]
        result[lane + "_prediction_used_pct"] = 100 * result[lane + "_used"] / total if total else None
        result[lane + "_evicted_unused_pct"] = 100 * result[lane + "_wasted"] / total if total else None
        result[lane + "_unused_at_end_pct"] = 100 * (total - result[lane + "_used"]) / total if total else None
    result["depths"] = sorted({int(r["depth"]) for r in rows})
    return result


if __name__ == "__main__":
    results = [summarize(sys.argv[1], "baseline"), summarize(sys.argv[2], "adaptive")]
    output = json.dumps(results, indent=2) + "\n"
    if len(sys.argv) > 3:
        pathlib.Path(sys.argv[3]).write_text(output, encoding="utf-8")
    print(output)
