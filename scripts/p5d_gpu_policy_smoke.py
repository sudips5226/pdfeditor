"""Compile and run the texture pin/eviction policy used by Direct3D (no GPU needed)."""
import os
from pathlib import Path
import subprocess

root = Path(__file__).resolve().parents[1]
out = root / 'artifacts/p5d-native-tests'
out.mkdir(parents=True, exist_ok=True)
vswhere = Path(os.environ.get('ProgramFiles(x86)', r'C:\Program Files (x86)')) / 'Microsoft Visual Studio/Installer/vswhere.exe'
install = subprocess.check_output([str(vswhere), '-latest', '-products', '*', '-requires',
                                   'Microsoft.VisualStudio.Component.VC.Tools.x86.x64',
                                   '-property', 'installationPath'], text=True).strip()
vcvars = Path(install) / 'VC/Auxiliary/Build/vcvars64.bat'
batch = out / 'run.cmd'
batch.write_text(f'@echo off\ncall "{vcvars}" >nul\n'
                 f'cl /nologo /std:c++20 /EHsc /W4 /WX "{root / "tests/gpu_presentation_policy.cpp"}" /Fe:gpu-policy.exe\n'
                 'if errorlevel 1 exit /b 1\ngpu-policy.exe\n', encoding='utf-8')
raise SystemExit(subprocess.call(['cmd', '/d', '/c', str(batch)], cwd=out,
                                env={k.upper(): v for k, v in os.environ.items()}))
