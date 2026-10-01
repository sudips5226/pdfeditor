"""Deterministic small vector PDF; optional large file stays outside fixtures."""
import pathlib
import sys


def generate(path, count=12):
    sizes = [(595, 842, 0), (1191, 842, 0), (842, 595, 90),
             (2400, 1800, 0), (600, 900, 270), (420, 595, 180)]
    objects = [b"<< /Type /Catalog /Pages 2 0 R >>", b"",
               b"<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>"]
    kids = []
    for i in range(count):
        width, height, rotation = sizes[i % len(sizes)]
        page = len(objects) + 1
        kids.append(f"{page} 0 R")
        objects.append((f"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 {width} {height}] "
                        f"/Rotate {rotation} /Resources << /Font << /F1 3 0 R >> >> "
                        f"/Contents {page + 1} 0 R >>").encode())
        content = (f"0.92 0.95 1 rg 0 0 {width} {height} re f\n"
                   f"0 0 0 RG 4 w 12 12 {width - 24} {height - 24} re S\n"
                   f"0 0 0 rg BT /F1 32 Tf 32 {height - 60} Td "
                   f"(P4 PAGE {i + 1} - {width} x {height} - rotation {rotation}) Tj ET\n"
                   f"1 0 0 rg 32 32 120 120 re f\n"
                   f"0 0.5 0 rg {width - 152} {height - 200} 120 120 re f\n").encode()
        objects.append(f"<< /Length {len(content)} >>\nstream\n".encode() + content + b"endstream")
    objects[1] = f"<< /Type /Pages /Count {count} /Kids [{' '.join(kids)}] >>".encode()
    data = bytearray(b"%PDF-1.7\n")
    offsets = [0]
    for i, obj in enumerate(objects, 1):
        offsets.append(len(data))
        data.extend(f"{i} 0 obj\n".encode() + obj + b"\nendobj\n")
    xref = len(data)
    data.extend(f"xref\n0 {len(offsets)}\n0000000000 65535 f \n".encode())
    for offset in offsets[1:]:
        data.extend(f"{offset:010d} 00000 n \n".encode())
    data.extend(f"trailer\n<< /Size {len(offsets)} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n".encode())
    pathlib.Path(path).write_bytes(data)


if __name__ == "__main__":
    generate(sys.argv[1], int(sys.argv[2]) if len(sys.argv) > 2 else 12)
