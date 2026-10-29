"""Export Demucs's htdemucs network to ONNX for Duet's stem splitter.

Everything between the spectrogram and the masking (both branches, the
transformer) goes into the ONNX file, at the model's fixed 7.8 s segment; the
STFT, its inverse and the overlap-add are in Duet's engine (engine/src/stems.rs).
Run by CI (a commit with [models] in its message), which publishes the file as
the release models-htdemucs-1.

    pip install torch torchaudio demucs==4.0.1 onnx
    python tools/export_demucs.py htdemucs.onnx
"""
import math, sys, torch, numpy as np
from einops import rearrange
from demucs.pretrained import get_model

bag = get_model("htdemucs")
m = bag.models[0].eval()
SR = m.samplerate
L = int(m.segment * SR)  # 343980
T = math.ceil(L / m.hop_length)

class Core(torch.nn.Module):
    def __init__(self, m):
        super().__init__()
        self.m = m
    def forward(self, mix, mag):
        self = self.m
        x = mag
        B, C, Fq, Tn = x.shape
        mean = x.mean(dim=(1, 2, 3), keepdim=True)
        std = x.std(dim=(1, 2, 3), keepdim=True)
        x = (x - mean) / (1e-5 + std)
        xt = mix
        meant = xt.mean(dim=(1, 2), keepdim=True)
        stdt = xt.std(dim=(1, 2), keepdim=True)
        xt = (xt - meant) / (1e-5 + stdt)
        saved, saved_t, lengths, lengths_t = [], [], [], []
        for idx, encode in enumerate(self.encoder):
            lengths.append(x.shape[-1])
            inject = None
            if idx < len(self.tencoder):
                lengths_t.append(xt.shape[-1])
                tenc = self.tencoder[idx]
                xt = tenc(xt)
                if not tenc.empty:
                    saved_t.append(xt)
                else:
                    inject = xt
            x = encode(x, inject)
            if idx == 0 and self.freq_emb is not None:
                frs = torch.arange(x.shape[-2], device=x.device)
                emb = self.freq_emb(frs).t()[None, :, :, None].expand_as(x)
                x = x + self.freq_emb_scale * emb
            saved.append(x)
        if self.crosstransformer:
            if self.bottom_channels:
                b, c, f, t = x.shape
                x = rearrange(x, "b c f t-> b c (f t)")
                x = self.channel_upsampler(x)
                x = rearrange(x, "b c (f t)-> b c f t", f=f)
                xt = self.channel_upsampler_t(xt)
            x, xt = self.crosstransformer(x, xt)
            if self.bottom_channels:
                x = rearrange(x, "b c f t-> b c (f t)")
                x = self.channel_downsampler(x)
                x = rearrange(x, "b c (f t)-> b c f t", f=f)
                xt = self.channel_downsampler_t(xt)
        for idx, decode in enumerate(self.decoder):
            skip = saved.pop(-1)
            x, pre = decode(x, skip, lengths.pop(-1))
            offset = self.depth - len(self.tdecoder)
            if idx >= offset:
                tdec = self.tdecoder[idx - offset]
                length_t = lengths_t.pop(-1)
                if tdec.empty:
                    pre = pre[:, :, 0]
                    xt, _ = tdec(pre, None, length_t)
                else:
                    skip = saved_t.pop(-1)
                    xt, _ = tdec(xt, skip, length_t)
        S = len(self.sources)
        x = x.view(B, S, -1, Fq, Tn)
        x = x * std[:, None] + mean[:, None]
        xt = xt.view(B, S, -1, mix.shape[-1])
        xt = xt * stdt[:, None] + meant[:, None]
        return x, xt

core = Core(m).eval()
mix = torch.randn(1, 2, L) * 0.1
z = m._spec(mix)
mag = m._magnitude(z)
print("shapes", mix.shape, mag.shape, "T", T)
with torch.no_grad():
    a, b = core(mix, mag)
    # the same through the model's own forward, to check the split is right
    full = m(mix)
    zout = m._mask(z, a)
    x = m._ispec(zout, L)
    mine = b + x
    diff = float((mine - full).abs().max())
    print("split matches forward:", diff)
    assert diff < 1e-4, "the split no longer matches the model's own forward"
out = sys.argv[1]
torch.onnx.export(core, (mix, mag), out, input_names=["mix", "mag"], output_names=["spec", "wave"], opset_version=17,
                  do_constant_folding=True)
print("exported", out)
