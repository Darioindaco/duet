# PyInstaller build for Windows (one-file Duet.exe) and macOS (Duet.app).
# Build the Rust engine into the environment first:  pip install ./engine
#   pyinstaller --noconfirm duet.spec
import sys

from PyInstaller.utils.hooks import collect_data_files

datas = [("ui", "ui")] + collect_data_files("webview")

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
            # macOS 15+ asks before an app may reach other devices on your Wi-Fi (a partner in the
            # same house); without a reason here that question can fail to appear at all.
            "NSLocalNetworkUsageDescription": "Duet connects straight to your partner's computer when you're on the same network.",
            "NSHighResolutionCapable": True,
            "CFBundleShortVersionString": "0.3.0",
            "LSMinimumSystemVersion": "11.0",
        },
    )
else:
    exe = EXE(pyz, a.scripts, a.binaries, a.datas, [], name="Duet", console=False, upx=False)
