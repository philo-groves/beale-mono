#!/usr/bin/env python3
"""Package prebuilt static Linux worker/Bash binaries; no guest installation."""
import argparse
import hashlib
import json
from pathlib import Path
import shutil

p = argparse.ArgumentParser()
p.add_argument("--worker", type=Path, required=True)
p.add_argument("--bash", type=Path, required=True)
p.add_argument("--bash-license", type=Path, required=True)
p.add_argument("--architecture", choices=["amd64", "arm64"], required=True)
p.add_argument("--output", type=Path, required=True)
args = p.parse_args()
args.output.mkdir(parents=True, exist_ok=False)
manifest = {"version": 1, "worker_protocol": 1, "architecture": args.architecture}
for key, source, name in [("worker", args.worker, "beale-worker"),
                          ("shell", args.bash, "bash"),
                          ("shell_license", args.bash_license, "BASH-COPYRIGHT")]:
    destination = args.output / name
    shutil.copyfile(source, destination)
    data = destination.read_bytes()
    manifest[key] = {"sha256": hashlib.sha256(data).hexdigest(), "bytes": len(data)}
(args.output / "manifest.json").write_text(json.dumps(manifest, indent=2) + "\n")
