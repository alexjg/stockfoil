#!/usr/bin/env python3
"""Check that representative semantic bugs are rejected by the full verifier."""
import os
from pathlib import Path
import shutil
import subprocess
import tempfile


ROOT = Path(__file__).resolve().parent.parent
# Last occurrence changes executable code while leaving its contract intact.
# The policy mutation deliberately changes the spec; a concrete witness must
# detect its disagreement with the intended out-of-history-frontier behavior.
MUTATIONS = [
    (
        "cap expands permission",
        "src/verified/clock.rs",
        "Some(limit) => Some(if tip <= limit { tip } else { limit }),",
        "Some(limit) => Some(if tip >= limit { tip } else { limit }),",
        "last",
    ),
    (
        "lookup excludes the inclusive tip",
        "src/verified/clock.rs",
        "Some(tip) => p <= tip,",
        "Some(tip) => p < tip,",
        "last",
    ),
    (
        "union takes the smaller position",
        "src/verified/clock.rs",
        "if left >= right { left } else { right }",
        "if left <= right { left } else { right }",
        "last",
    ),
    (
        "new invisible commits are omitted",
        "src/verified/events.rs",
        "newly_delivered || matches!(visibility, Vis::Before | Vis::After)",
        "matches!(visibility, Vis::Before | Vis::After)",
        "last",
    ),
    (
        "permission spec accepts out-of-history frontier heads",
        "verification/visibility.rs",
        "frontier.contains(f) && h.contains(f) && reaches(g, a, f)",
        "frontier.contains(f) && reaches(g, a, f)",
        "first",
    ),
]


def verify(root):
    return subprocess.run(
        ["bash", str(root / "verification/verify.sh")],
        cwd=root,
        env=os.environ.copy(),
        capture_output=True,
        text=True,
        timeout=120,
    )


def main():
    baseline = verify(ROOT)
    if baseline.returncode:
        raise SystemExit("Baseline failed:\n" + baseline.stdout + baseline.stderr)
    print(baseline.stdout.strip(), flush=True)
    for name, relative, before, after, occurrence in MUTATIONS:
        with tempfile.TemporaryDirectory(prefix="stockfoil-verus-") as directory:
            root = Path(directory)
            shutil.copytree(ROOT / "src/verified", root / "src/verified")
            shutil.copytree(ROOT / "verification", root / "verification")
            path = root / relative
            source = path.read_text()
            split = source.rpartition if occurrence == "last" else source.partition
            prefix, matched, suffix = split(before)
            if not matched:
                raise SystemExit(f"Mutation no longer matches source: {name}")
            path.write_text(prefix + after + suffix)
            result = verify(root)
            output = result.stdout + result.stderr
            if result.returncode == 0 or not any(
                message in output
                for message in ("assertion failed", "postcondition not satisfied")
            ):
                raise SystemExit(f"Expected a proof failure for {name}:\n{output}")
            print(f"Rejected: {name}", flush=True)


if __name__ == "__main__":
    main()
