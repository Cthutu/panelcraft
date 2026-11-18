"""Package the native release binary and end-user files into a ZIP."""

import json
import os
from pathlib import Path
import subprocess
import tempfile
from zipfile import ZIP_DEFLATED, ZipFile


ROOT = Path(__file__).resolve().parents[1]


def main():
    host = next(
        line.split(": ", 1)[1]
        for line in subprocess.check_output(["rustc", "-vV"], text=True).splitlines()
        if line.startswith("host: ")
    )
    build = subprocess.run(
        ["cargo", "build", "--locked", "--release", "--bin", "panelcraft",
         "--target", host, "--message-format=json-render-diagnostics"],
        cwd=ROOT, stdout=subprocess.PIPE, text=True, check=True,
    )
    artifacts = map(json.loads, build.stdout.splitlines())
    executable = next(
        Path(item["executable"]) for item in artifacts
        if item["reason"] == "compiler-artifact"
        and item["target"]["name"] == "panelcraft" and item.get("executable")
    )
    metadata = json.loads(subprocess.check_output(
        ["cargo", "metadata", "--locked", "--format-version=1", "--no-deps"], cwd=ROOT,
    ))
    version = next(pkg["version"] for pkg in metadata["packages"] if pkg["name"] == "panelcraft")
    output = ROOT / "dist" / f"panelcraft-{version}-{host}.zip"
    output.parent.mkdir(exist_ok=True)
    with tempfile.NamedTemporaryFile(dir=output.parent, delete=False) as staging:
        temporary = Path(staging.name)
    try:
        with ZipFile(temporary, "w", compression=ZIP_DEFLATED) as archive:
            archive.write(executable, executable.name)
            archive.write(ROOT / "index.html", "index.html")
            archive.write(ROOT / "docs" / "distribution.md", "README.md")
        os.replace(temporary, output)
    finally:
        temporary.unlink(missing_ok=True)
    print(output)


if __name__ == "__main__":
    main()
