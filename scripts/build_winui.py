"""Local MSBuild wrapper: canonicalize case-insensitive Windows environment keys."""
import os
import pathlib
import subprocess
import sys

environment = {key.upper(): value for key, value in os.environ.items()}
environment["MSBUILDDISABLENODEREUSE"] = "1"
msbuild = pathlib.Path(os.environ.get("ProgramFiles", r"C:\Program Files")) / (
    r"Microsoft Visual Studio\18\Community\MSBuild\Current\Bin\MSBuild.exe")
if len(sys.argv) > 1:
    msbuild = pathlib.Path(sys.argv[1])
raise SystemExit(subprocess.call([str(msbuild), "native/winui/PdfEditor.WinUI.vcxproj",
    "/m", "/nr:false", "/p:Configuration=Debug", "/p:Platform=x64", "/v:minimal", *sys.argv[2:]], env=environment))
