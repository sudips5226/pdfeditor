"""Small distinguishable structural-preservation fixtures. No external libraries."""
from pathlib import Path
import sys

def generate(path, external=False):
    objects = [b'<< /Type /Catalog /Pages 2 0 R >>', b'',
               b'<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>']
    kids = []
    for i in range(3 if external else 6):
        width, height = (460 + i * 37, 640 + i * 29) if external else (600 + i * 31, 800 + i * 23)
        rotation = [0,90,270,180,0,90][i]
        page = len(objects) + 1; kids.append(f'{page} 0 R')
        marker = ('EXTERNAL' if external else 'PRIMARY') + f' {i+1}'
        objects.append((f'<< /Type /Page /Parent 2 0 R /MediaBox [0 0 {width} {height}] '
                        f'/CropBox [10 20 {width-15} {height-25}] /Rotate {rotation} '
                        f'/Resources << /Font << /F1 3 0 R >> >> /Contents {page+1} 0 R '
                        f'/Annots [{page+2} 0 R] >>').encode())
        content = (f'0 0 0 rg BT /F1 28 Tf 40 {height-80} Td ({marker}) Tj ET\n'
                   f'{.1+i*.1:.1f} .3 .6 rg 40 50 {100+i*20} {120+i*9} re f\n').encode()
        objects.append(f'<< /Length {len(content)} >>\nstream\n'.encode() + content + b'endstream')
        objects.append((f'<< /Type /Annot /Subtype /Text /Rect [40 50 60 70] /Contents ({marker} NOTE) '
                        f'/P {page} 0 R /Name /Comment >>').encode())
    objects[1] = f"<< /Type /Pages /Count {len(kids)} /Kids [{' '.join(kids)}] >>".encode()
    data = bytearray(b'%PDF-1.7\n'); offsets=[0]
    for i, obj in enumerate(objects,1):
        offsets.append(len(data));data.extend(f'{i} 0 obj\n'.encode()+obj+b'\nendobj\n')
    xref=len(data);data.extend(f'xref\n0 {len(offsets)}\n0000000000 65535 f \n'.encode())
    for offset in offsets[1:]:data.extend(f'{offset:010d} 00000 n \n'.encode())
    data.extend(f'trailer\n<< /Size {len(offsets)} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n'.encode())
    Path(path).parent.mkdir(parents=True,exist_ok=True);Path(path).write_bytes(data)
if __name__ == '__main__':
    generate(sys.argv[1], len(sys.argv)>2 and sys.argv[2]=='external')
