"""Reject dependencies that require an external local checkout."""
import json
from pathlib import Path
import subprocess


root = Path(__file__).resolve().parents[1]
metadata = json.loads(subprocess.check_output(
    ["cargo", "metadata", "--locked", "--format-version", "1"],
    cwd=root, text=True,
))
local = [
    package["manifest_path"] for package in metadata["packages"]
    if package["source"] is None and package["id"] != metadata["resolve"]["root"]
]
if local:
    raise SystemExit("External local dependencies are not allowed:\n" + "\n".join(local))

for package in metadata["packages"]:
    if package["source"] and package["source"].startswith("git+"):
        print(f"{package['name']}: {package['source']}")
print("Dependency sources OK: only this project is local; all dependencies use Git or a registry")
