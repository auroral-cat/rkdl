"""Write LICENSE-THIRD-PARTY: the licenses of every Rust crate linked into rkdl's extension, plus their NOTICE files.

Needs cargo-about (https://github.com/EmbarkStudios/cargo-about). The crate list comes from Cargo.lock (`--locked`);
which license of an "A OR B" crate gets listed follows the order in about.toml. Apache-2.0 asks redistributors to
carry a crate's NOTICE file, so any crate shipping one gets it copied in verbatim.

Usage: uv run scripts/third_party_licenses.py [output path, default LICENSE-THIRD-PARTY]
"""

import json
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
RULE = "=" * 79
HEADER = """\
Third-party software in rkdl

rkdl's compiled extension statically links the Rust crates below. Each section is one license text and the crates
distributed under it; crates offering a choice of licenses are listed under one of them. NOTICE files the crates ship
follow the licenses.

Generated from Cargo.lock by cargo-about and scripts/third_party_licenses.py.
"""


def main() -> None:
    output = Path(sys.argv[1]) if len(sys.argv) > 1 else ROOT / "LICENSE-THIRD-PARTY"
    about = subprocess.run(
        ["cargo", "about", "generate", "--format", "json", "--locked", "-c", str(ROOT / "about.toml")],
        cwd=ROOT,
        check=True,
        stdout=subprocess.PIPE,
        text=True,
    )
    data = json.loads(about.stdout)

    def is_rkdl(package: dict) -> bool:
        return package["source"] is None and package["name"] == "rkdl"

    parts = [HEADER]
    for license in data["licenses"]:
        users = sorted(
            {f"{u['crate']['name']} {u['crate']['version']}" for u in license["used_by"] if not is_rkdl(u["crate"])},
        )
        if not users:
            continue
        parts.append(f"{RULE}\n{license['name']} ({license['id']})\nUsed by: {', '.join(users)}\n{RULE}\n\n")
        parts.append(license["text"].strip("\n") + "\n")

    notices = 0
    for krate in sorted(data["crates"], key=lambda c: (c["package"]["name"], c["package"]["version"])):
        package = krate["package"]
        if is_rkdl(package):
            continue
        crate_dir = Path(package["manifest_path"]).parent
        for notice in sorted(p for p in crate_dir.iterdir() if p.is_file() and p.name.upper().startswith("NOTICE")):
            parts.append(f"{RULE}\nNOTICE file from {package['name']} {package['version']} ({notice.name})\n{RULE}\n\n")
            parts.append(notice.read_text(encoding="utf-8").strip("\n") + "\n")
            notices += 1

    output.write_text("\n".join(parts), encoding="utf-8", newline="\n")
    crates = sum(1 for c in data["crates"] if not is_rkdl(c["package"]))
    print(f"wrote {output}: {crates} crates, {notices} NOTICE file(s)")


if __name__ == "__main__":
    main()
