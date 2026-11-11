#!/usr/bin/env python3
"""Collect Criterion results for one saved baseline into a JSON summary.

Usage: bench/collect.py <baseline-name> <output.json>

Reads target/criterion/**/<baseline>/estimates.json and benchmark.json and
writes one record per benchmark (mean and median with 95% confidence bounds,
in nanoseconds), together with machine, toolchain, and git commit, so results
can be compared across stages.
"""

import json
import pathlib
import platform
import subprocess
import sys


def run(*cmd: str) -> str:
    try:
        return subprocess.run(cmd, capture_output=True, text=True, check=True).stdout.strip()
    except (OSError, subprocess.CalledProcessError):
        return "unknown"


def cpu_model() -> str:
    try:
        for line in pathlib.Path("/proc/cpuinfo").read_text().splitlines():
            if line.startswith("model name"):
                return line.split(":", 1)[1].strip()
    except OSError:
        pass
    return platform.processor() or "unknown"


def main() -> None:
    if len(sys.argv) != 3:
        sys.exit(__doc__)
    baseline, output = sys.argv[1], pathlib.Path(sys.argv[2])
    root = pathlib.Path("target/criterion")
    records = []
    for est_path in sorted(root.glob(f"**/{baseline}/estimates.json")):
        meta_path = est_path.with_name("benchmark.json")
        meta = json.loads(meta_path.read_text()) if meta_path.exists() else {}
        est = json.loads(est_path.read_text())

        def stat(name: str) -> dict[str, float]:
            s = est[name]
            ci = s["confidence_interval"]
            return {"point_ns": s["point_estimate"], "lower_ns": ci["lower_bound"], "upper_ns": ci["upper_bound"]}

        records.append({
            "id": meta.get("full_id", str(est_path.parent.parent.relative_to(root))),
            "throughput": meta.get("throughput"),
            "mean": stat("mean"),
            "median": stat("median"),
        })
    summary = {
        "baseline": baseline,
        "commit": run("git", "rev-parse", "--short", "HEAD"),
        "rustc": run("rustc", "--version"),
        "cpu": cpu_model(),
        "os": f"{platform.system()} {platform.release()}",
        "profile": "bench (release, debug symbols)",
        "results": records,
    }
    output.write_text(json.dumps(summary, indent=2) + "\n")
    print(f"wrote {len(records)} results to {output}")


if __name__ == "__main__":
    main()
