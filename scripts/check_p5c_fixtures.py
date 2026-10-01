"""Check that P5C PDF fixtures survived checkout without byte conversion."""

from pathlib import Path
import subprocess


ROOT = Path(__file__).resolve().parents[1]
FIXTURES = (
    "tests/fixtures/p5c-primary.pdf",
    "tests/fixtures/p5c-external.pdf",
)


def check_fixture(name: str) -> None:
    path = ROOT / name
    data = path.read_bytes()
    committed = subprocess.check_output(["git", "show", f"HEAD:{name}"], cwd=ROOT)
    if data != committed:
        raise ValueError(f"{name}: checkout bytes differ from the committed blob")
    if not data.startswith(b"%PDF-"):
        raise ValueError(f"{name}: missing PDF header")
    marker = b"startxref\n"
    marker_at = data.rfind(marker)
    if marker_at < 0:
        raise ValueError(f"{name}: missing LF-terminated startxref")
    declared = int(data[marker_at + len(marker) :].split(None, 1)[0])
    actual = data.find(b"xref\n")
    if actual < 0 or declared != actual:
        raise ValueError(f"{name}: startxref {declared}, actual xref {actual}")
    print(f"{name}: {len(data)} bytes, startxref = xref byte {actual}, checkout matches Git")


if __name__ == "__main__":
    for fixture in FIXTURES:
        check_fixture(fixture)
