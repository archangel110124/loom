#!/usr/bin/env python3
"""Generate `assets/audio/sea.wav` — the hub's ambient sea wash.

Run: `python3 scripts/make-sea-wav.py`. Deterministic (fixed seed), so it
rewrites the same bytes every time; the file is checked in because the engine
has no runtime synthesiser for anything but rain and a demo should not need a
build step to have a sea in it.

**Why it is synthesised and not recorded.** There is no sea recording in this
repository and there are exactly two clips in it: `hum.wav` and `rain.wav`.
Reusing the rain bed as a sea would be a lie in the scene file that nobody
could hear was one.

**Why it loops without a seam.** The noise is built in the frequency domain
with `irfft` over exactly the loop length, so every component is an integer
harmonic of the loop period and the last sample joins the first by
construction — no crossfade, no click. The swell envelope is built the same
way, out of the first three harmonics, so the breathing loops too.

`AudioSource` has no pitch or filter knob and a node script may write only
`position`/`rotation`/`scale`, so everything the ear is meant to hear about
this sound other than its distance has to be baked in here.
"""

import struct
import wave

import numpy as np

RATE = 22050
SECONDS = 8.0
OUT = "assets/audio/sea.wav"

n = int(RATE * SECONDS)
rng = np.random.default_rng(20260820)

# Random-phase spectrum, shaped. `f0` is the loop's fundamental, so bin k is
# exactly k cycles per loop.
freqs = np.fft.rfftfreq(n, 1.0 / RATE)
mag = np.zeros_like(freqs)
nz = freqs > 0
# Pink-ish body with a shelf: sea wash is broad, weighted low, and rolls off
# above a couple of kilohertz. 1/f^0.9 with a first-order low pass at 1.4 kHz
# and a gentle high-pass at 90 Hz, which is where a small speaker gives up and
# where rumble starts sounding like a lorry rather than water.
mag[nz] = freqs[nz] ** -0.9
mag[nz] /= np.sqrt(1.0 + (freqs[nz] / 1400.0) ** 2)
mag[nz] *= freqs[nz] ** 2 / (freqs[nz] ** 2 + 90.0**2)
phase = rng.uniform(0.0, 2.0 * np.pi, freqs.shape)
sig = np.fft.irfft(mag * np.exp(1j * phase), n)

# The swell. Three harmonics of the loop, so an 8 s, a 4 s and a 2.67 s breath
# beating against each other — flat noise reads as static, and one sine reads
# as a machine.
t = np.arange(n) / RATE
swell = (
    0.50
    + 0.30 * np.sin(2 * np.pi * t / SECONDS + 0.3)
    + 0.14 * np.sin(4 * np.pi * t / SECONDS + 1.9)
    + 0.06 * np.sin(6 * np.pi * t / SECONDS + 4.1)
)
sig *= swell

sig /= np.max(np.abs(sig))
sig *= 0.85
pcm = np.clip(sig * 32767.0, -32768, 32767).astype("<i2")

with wave.open(OUT, "wb") as w:
    w.setnchannels(1)
    w.setsampwidth(2)
    w.setframerate(RATE)
    w.writeframes(pcm.tobytes())

rms = float(np.sqrt(np.mean((pcm.astype(np.float64) / 32768.0) ** 2)))
seam = abs(int(pcm[0]) - int(pcm[-1])) / 32768.0
print(f"{OUT}: {n} frames, {SECONDS:.1f}s, rms {rms:.3f}, seam step {seam:.4f}")
assert struct.calcsize("<h") == 2
