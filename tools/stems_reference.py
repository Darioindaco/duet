"""Reference for engine/src/stems.rs's test (matches_pytorch_demucs): a made-up 20 s mix (kick, bass, hats, a 'voice'), separated by PyTorch
Demucs the way Duet does it (no random shifts, 25% overlap)."""
import numpy as np, torch, sys, wave
from demucs.pretrained import get_model
from demucs.apply import apply_model
sr, secs = 44100, 20.0
t = np.arange(int(sr * secs)) / sr
rng = np.random.default_rng(1)
kick = np.zeros_like(t); hats = np.zeros_like(t)
for b in np.arange(0, secs, 0.5):
    i = int(b * sr); n = int(0.25 * sr)
    tt = np.arange(n) / sr
    kick[i:i + n] += np.sin(2 * np.pi * (50 + 80 * np.exp(-tt / 0.03)) * tt) * np.exp(-tt / 0.12)
for b in np.arange(0.25, secs, 0.5):
    i = int(b * sr); n = int(0.05 * sr)
    hats[i:i + n] += rng.standard_normal(n) * np.exp(-np.arange(n) / (0.01 * sr)) * 0.3
bass = 0.4 * np.sin(2 * np.pi * 55 * t) * (0.6 + 0.4 * np.sin(2 * np.pi * 0.5 * t))
f0 = 220 * 2 ** (np.floor(t) % 5 / 12)
voice = 0.25 * np.sin(np.cumsum(2 * np.pi * f0 / sr)) * (np.sin(2 * np.pi * 3 * t) > -0.5)
L = 0.7 * kick + bass + hats + voice
R = 0.7 * kick + bass + 0.6 * hats + voice
mix = np.stack([L, R]).astype(np.float32) * 0.5
out = sys.argv[1]
mix.T.astype("<f4").tofile(out + "/ref_mix.bin")
with wave.open(out + "/ref_mix.wav", "wb") as w:
    w.setnchannels(2); w.setsampwidth(2); w.setframerate(sr)
    w.writeframes((np.clip(mix.T, -1, 1) * 32767).astype("<i2").tobytes())
m = get_model("htdemucs")
wav = torch.from_numpy(mix)
ref = wav.mean(0)
wav = (wav - ref.mean()) / ref.std()
with torch.no_grad():
    src = apply_model(m, wav[None], shifts=0, split=True, overlap=0.25, progress=False)[0]
src = src * ref.std() + ref.mean()
src.numpy().astype("<f4").tofile(out + "/ref_stems.bin")  # [4 sources, 2, n]
print("sources", m.sources, src.shape, [round(float(src[i].abs().max()), 3) for i in range(4)])
