#!/usr/bin/env python3
"""Generate one seamless photoreal ground texture set with ComfyUI.

albedo  -> RealVisXL with the UNet's conv layers patched to circular padding,
           which makes the tile mathematically seamless rather than blended.
normal  -> DeepBump "Color to Normals", whose convolutions wrap, so a seamless
           albedo yields a seamless normal.

Usage: gen_ground.py NAME "prompt" [SEED]
"""
import json
import sys
import time
import urllib.request

HOST = "http://127.0.0.1:8188"
CKPT = "RealVisXL_V5.0_fp16.safetensors"
SIZE = 1024

# Flat, even, overcast light. This is not a style preference: any directional
# shadow baked into the albedo becomes a permanent dent once DeepBump reads
# relief out of it, and becomes a lie the moment the engine lights the surface
# from somewhere else.
NEG = (
    "shadows, harsh lighting, sun, directional light, vignette, blurry, "
    "depth of field, bokeh, perspective, horizon, sky, plant, tree, object, "
    "watermark, text, border, frame, tiling seam, illustration, painting, "
    "cartoon, 3d render, cgi"
)


def post(path: str, payload: dict) -> dict:
    req = urllib.request.Request(
        HOST + path,
        data=json.dumps(payload).encode(),
        headers={"Content-Type": "application/json"},
    )
    with urllib.request.urlopen(req, timeout=60) as r:
        return json.loads(r.read())


def get(path: str) -> dict:
    with urllib.request.urlopen(HOST + path, timeout=60) as r:
        return json.loads(r.read())


def graph(name: str, prompt: str, seed: int, extra_neg: str = "") -> dict:
    # **Exclusions belong in the negative prompt, not the positive one.**
    # "absolutely no cracks" in the positive prompt produced cracked mud twice:
    # CLIP has no negation, so the token "cracks" only ever adds cracks.
    neg = NEG + (", " + extra_neg if extra_neg else "")
    return {
        "1": {"class_type": "CheckpointLoaderSimple",
              "inputs": {"ckpt_name": CKPT}},
        # **Slot 1, not 0.** This node returns (passthrough, patched); taking
        # slot 0 silently generates an ordinary non-tiling image and every
        # downstream check still passes except the one that matters.
        "2": {"class_type": "Model Patch Seamless (mtb)",
              "inputs": {"model": ["1", 0], "startStep": 0, "stopStep": 999,
                         "tilingX": True, "tilingY": True}},
        "3": {"class_type": "CLIPTextEncode",
              "inputs": {"clip": ["1", 1], "text": prompt}},
        "4": {"class_type": "CLIPTextEncode",
              "inputs": {"clip": ["1", 1], "text": neg}},
        "5": {"class_type": "EmptyLatentImage",
              "inputs": {"width": SIZE, "height": SIZE, "batch_size": 1}},
        "6": {"class_type": "KSampler",
              "inputs": {"model": ["2", 1], "positive": ["3", 0],
                         "negative": ["4", 0], "latent_image": ["5", 0],
                         "seed": seed, "steps": 30, "cfg": 4.5,
                         "sampler_name": "dpmpp_2m", "scheduler": "karras",
                         "denoise": 1.0}},
        # **The VAE must be patched too, and this is the whole ballgame.**
        # Patching only the UNet produced a texture that scored z = +24 on the
        # seam test — statistically identical to a random photo crop — because
        # the decoder's own conv layers re-introduce the edge the UNet avoided.
        # `use_tiling_decoder` must be off: the node refuses to combine them.
        "7": {"class_type": "Vae Decode (mtb)",
              "inputs": {"samples": ["6", 0], "vae": ["1", 2],
                         "seamless_model": True, "use_tiling_decoder": False,
                         "tile_size": 512}},
        "8": {"class_type": "SaveImage",
              "inputs": {"images": ["7", 0], "filename_prefix": f"loom/{name}_albedo"}},
        "9": {"class_type": "Deep Bump (mtb)",
              "inputs": {"image": ["7", 0], "mode": "Color to Normals",
                         "color_to_normals_overlap": "LARGE",
                         "normals_to_curvature_blur_radius": "MEDIUM",
                         "normals_to_height_seamless": True,
                         "auto_download": True}},
        "10": {"class_type": "SaveImage",
               "inputs": {"images": ["9", 0], "filename_prefix": f"loom/{name}_normal"}},
    }


def run(name: str, prompt: str, seed: int, extra_neg: str = "") -> list[str]:
    pid = post("/prompt", {"prompt": graph(name, prompt, seed, extra_neg)})["prompt_id"]
    print(f"queued {name} as {pid}", flush=True)
    while True:
        h = get(f"/history/{pid}")
        if pid in h:
            break
        time.sleep(3)
    out = []
    for node in h[pid]["outputs"].values():
        for img in node.get("images", []):
            out.append(f"{img['subfolder']}/{img['filename']}")
    return out


if __name__ == "__main__":
    n, p = sys.argv[1], sys.argv[2]
    s = int(sys.argv[3]) if len(sys.argv) > 3 else 12345
    xn = sys.argv[4] if len(sys.argv) > 4 else ""
    for f in run(n, p, s, xn):
        print("  wrote", f)
