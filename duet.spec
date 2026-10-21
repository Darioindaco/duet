# PyInstaller build for Windows (one-file Duet.exe) and macOS (Duet.app).
#   pyinstaller --noconfirm duet.spec
import sys

from PyInstaller.utils.hooks import collect_data_files

datas = [("ui", "ui")] + collect_data_files("_sounddevice_data") + collect_data_files("webview")

a = Analysis(["duet.py"], datas=datas)
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
            "CFBundleShortVersionString": "0.2.0",
            "LSMinimumSystemVersion": "11.0",
        },
    )
else:
    exe = EXE(pyz, a.scripts, a.binaries, a.datas, [], name="Duet", console=False, upx=False)
