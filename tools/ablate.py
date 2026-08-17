#!/usr/bin/env python3
"""Amplified difference between a scene and the same scene with `[ripples]`
stripped — what the wake alone is worth on screen.

usage: tools/ablate.py <scene.loom> <tick> [gain] [size]
Writes /tmp/ablate_{with,without,diff}.png and prints the mean and fraction.
"""
import subprocess, sys, tempfile, os
import numpy as np
from PIL import Image

scene, tick = sys.argv[1], sys.argv[2]
gain = float(sys.argv[3]) if len(sys.argv) > 3 else 4.0
size = sys.argv[4] if len(sys.argv) > 4 else "640x400"

src = open(scene).read()
i = src.index("[node.components.WaterBody.ripples]")
j = src.index("\n[[node]]", i)
stripped = tempfile.NamedTemporaryFile("w", suffix=".loom", delete=False)
stripped.write(src[:i] + src[j:])
stripped.close()

def render(path, out):
    subprocess.run(["./target/release/loom", "render", path, "--sim", tick,
                    "--size", size, "--out", out], check=True, stdout=subprocess.DEVNULL)

render(scene, "/tmp/ablate_with.png")
render(stripped.name, "/tmp/ablate_without.png")
os.unlink(stripped.name)

a = np.asarray(Image.open("/tmp/ablate_with.png").convert("RGB"), dtype=np.int16)
b = np.asarray(Image.open("/tmp/ablate_without.png").convert("RGB"), dtype=np.int16)
d = np.abs(a - b)
print(f"mean {d.mean():.3f}   fraction>0 {(d.max(axis=2) > 0).mean():.3f}   worst {d.max()}")
Image.fromarray(np.clip(d * gain, 0, 255).astype(np.uint8)).save("/tmp/ablate_diff.png")
