# PyInstaller build for Windows (a Duet folder with Duet.exe, which installer/duet.iss turns into
# Duet-setup.exe) and macOS (Duet.app).
# Build the Rust engine into the environment first:  pip install ./engine
#   pyinstaller --noconfirm duet.spec
# CI puts build_info.json next to this file first: the build's number, for the updater.
import json
import os
import sys

from PyInstaller.utils.hooks import collect_data_files

datas = [("ui", "ui")] + collect_data_files("webview")
run = None
if os.path.exists("build_info.json"):
    datas.append(("build_info.json", "."))
    run = json.load(open("build_info.json")).get("run")

a = Analysis(["duet.py"], datas=datas, hiddenimports=["duet_engine"])
pyz = PYZ(a.pure)

if sys.platform == "darwin":
    exe = EXE(pyz, a.scripts, [], exclude_binaries=True, name="Duet", console=False,
              argv_emulation=False, target_arch=None)
    coll = COLLECT(exe, a.binaries, a.datas, name="Duet")
    app = BUNDLE(
        coll,
        name="Duet.app",
        bundle_identifier="app.duet.jam",
        info_plist={
            # Without this key macOS silently feeds the app silence instead of the mic.
            "NSMicrophoneUsageDescription": "Duet sends your instrument or voice to your partner.",
            # Same for the camera, except macOS kills the app outright when it's missing.
            "NSCameraUsageDescription": "Duet shows you to your partner in the video call.",
            "NSHighResolutionCapable": True,
            "CFBundleShortVersionString": f"0.3.{run or 0}",
            "CFBundleVersion": str(run or 0),
            "LSMinimumSystemVersion": "11.0",
        },
    )
else:
    # a folder (dist/Duet) rather than one self-unpacking exe: it starts right away, and the
    # installer puts it in a program folder like any other app
    exe = EXE(pyz, a.scripts, [], exclude_binaries=True, name="Duet", console=False, upx=False)
    coll = COLLECT(exe, a.binaries, a.datas, name="Duet", upx=False)
