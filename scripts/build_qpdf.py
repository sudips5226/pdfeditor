"""Download pinned official SDK, verify SHA-256, build private x64 C adapter.

No qpdf executable is invoked. The SDK binaries remain in ignored build/.
"""
import hashlib
import os
from pathlib import Path
import shutil
import subprocess
import urllib.request
import zipfile

ROOT = Path(__file__).resolve().parents[1]
VERSION = '12.3.2'
SHA256 = '8941870a604e7c87ed24566b038d46c24ce76616254d2383c578f60c0677f202'
ARCHIVE = ROOT / 'build' / f'qpdf-{VERSION}-msvc64.zip'
SDK = ROOT / 'build/qpdf' / f'qpdf-{VERSION}-msvc64'
OUT = ROOT / 'build/structural'
def main():
    ARCHIVE.parent.mkdir(exist_ok=True)
    if not ARCHIVE.exists():
        urllib.request.urlretrieve(f'https://github.com/qpdf/qpdf/releases/download/v{VERSION}/{ARCHIVE.name}', ARCHIVE)
    with ARCHIVE.open('rb') as f:
        if hashlib.file_digest(f, 'sha256').hexdigest() != SHA256:
            raise RuntimeError('qpdf SDK checksum mismatch')
    if not SDK.exists():
        with zipfile.ZipFile(ARCHIVE) as z:
            z.extractall(SDK.parent)
    OUT.mkdir(exist_ok=True)
    vswhere = Path(os.environ.get('ProgramFiles(x86)', r'C:\Program Files (x86)')) / 'Microsoft Visual Studio/Installer/vswhere.exe'
    install = subprocess.check_output([str(vswhere), '-latest', '-products', '*', '-requires', 'Microsoft.VisualStudio.Component.VC.Tools.x86.x64', '-property', 'installationPath'], text=True).strip()
    vcvars = Path(install) / 'VC/Auxiliary/Build/vcvars64.bat'
    command = f'call "{vcvars}" >nul && cl /nologo /std:c++17 /MD /EHsc /O2 /W4 /WX /LD /external:I"{SDK / "include"}" /external:W0 "{ROOT / "native/qpdf-adapter/adapter.cpp"}" /link "{SDK / "lib/qpdf.lib"}" /OUT:pdfeditor_qpdf.dll'
    environment = {k.upper():v for k,v in os.environ.items()}
    batch = OUT / 'compile.cmd'
    batch.write_text('@echo off\n' + command + '\n', encoding='utf-8')
    subprocess.run(f'cmd /d /c "{batch}"', cwd=OUT, env=environment, check=True)
    for dll in (SDK / 'bin').glob('*.dll'):
        shutil.copy2(dll, OUT / dll.name)
    # SDK documentation includes redistributable copyright/license notices.
    notices = SDK / 'share/doc/qpdf'
    shutil.copytree(notices, OUT / 'qpdf-notices', dirs_exist_ok=True)
    for notice in (ROOT / 'third_party/qpdf').glob('*'):
        shutil.copy2(notice, OUT / 'qpdf-notices' / notice.name)
    print(f'Built libqpdf {VERSION} structural adapter: {OUT}')
if __name__ == '__main__':
    main()
