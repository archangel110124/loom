# What Loom needs from the machine it runs on

This project's gates are **machine-local by design** — ADR 0053 says so in
writing, and ADR 0073 builds on it. That makes the host a dependency, and an
undocumented one until now. This file records what the host has to provide and
what the references were blessed against, so that "did the machine change this?"
is answerable rather than a guess.

## The baseline the current references were blessed on

    cpu           AMD Ryzen 7 9800X3D, 8 cores, SMT off (1 thread/core)
    kernel        7.1.6-201.fc44.x86_64
    distro        Fedora Linux 44
    session       X11 / KDE
    gpu           NVIDIA GeForce RTX 4090, capped to 300 W
    nvidia driver 610.57.04
    vulkan api    1.4.341
    rustc         1.97.1
    slangc        2026.14.1
    blender       5.2.0 LTS
    python        3.14.6

**The GPU driver is the number that matters most.** It, not the CPU, is what a
reference PNG is really a function of.

## The non-obvious dependency

**`slangc` is a hand-installed binary at `~/.local/bin/slangc`, not a package.**
`crates/loom_render/build.rs` shells out to it to compile every shader. On a
fresh machine it will not exist and nothing will build. Version **2026.14.1**;
upstream is `shader-slang/slang` releases. A copy is in the migration bundle.

`build.rs` **skips `slangc` when no shader input is newer than the `.spv` it
already emitted**, and it dates `slangc` itself among its inputs. So a new
`slangc` correctly forces a rebuild — but if a shader edit ever appears not to
reach the GPU, that skip is the first place to look.

## What a hardware change actually does — measured, 2026-08-22

The case, motherboard, CPU and cooler were replaced. GPU and driver unchanged.
Measured immediately after:

- **Every pinned determinism hash held.** Full workspace suite, zero failures.
  Different CPU, same simulation, bit for bit.
- **53 of 54 render rows reproduced their pre-swap measurement essentially
  exactly** — `campfire` 103, `gleamsprat_beat` 81, `plume_roof` 76 to the digit;
  `slosh` 85 → 86, one unit, at a single CMAA2 edge pixel.
- **One row moved by 58.** `plough_cinematic_low` went from worst-channel 36 to
  94. It is a **cinematic-fluid** scene, and ADR 0053 declares that tier's
  reproducibility **machine-local**. This is the first empirical evidence for a
  claim that ADR had been making on reasoning alone.

**So: the deterministic core survives a CPU change; the cinematic tier does not,
exactly as documented.** Expect a driver change to move considerably more, since
the references are a function of the driver in a way they are not of the CPU.

## Host requirements

**Vulkan 1.3 minimum** (the loader here reports 1.4; the target is 1.3), with
ray-query support — `sunVisibility`, `ambientVisibility` and `tracedEnvironment`
are inline ray queries against a TLAS (ADR 0019). Without them the engine does
not render as blessed.

**Validation layers must be installed and working.** `cargo xtask validate`
requires `VK_LAYER_KHRONOS_validation`; debug builds **panic** on a validation
message by design and that must not be downgraded.

**A real display.** `loom run` opens a window and there is **no Xvfb here**, so
anything that needs the window — the HUD, the pause menu, the inventory, the
title screen, `loom run --shot` — needs a session. `loom render` is headless and
covers everything else.

**Build with `-j 3`, not `-j 6`.** The reason is memory pressure during
compilation, not core count; the box swapped at higher parallelism. That was
measured on the previous CPU with the same 60 GB, so it is still believed to
hold, but it is worth re-measuring rather than inheriting.

## External tools the project actually uses

    blender      headless asset builds (--background --factory-startup --python)
    imagemagick  `magick montage` for contact sheets — how frame sequences are read
    ffmpeg       frame sequences to GIF
    python3      numpy/scipy for measurement harnesses

## System configuration that is deliberate, not accidental

- **The GPU is capped to 300 W** by `/etc/systemd/system/nvidia-pl.service`
  against a 450 W default. This is for room heat and is intentional. Do not
  raise it; a temporary `nvidia-smi -pl` is the reversible alternative if a
  measurement genuinely needs it.
- Swap is **zram**, not a disk partition. `swapoff -a` does not do what you
  expect here and `swapon -a` cannot undo it — there is no fstab entry to
  restore from.
- `/home` and `/mnt/data` are **different physical drives**. `/mnt/data` is the
  3.6 TB Crucial and survives a reinstall of `/home`.

## Where the truth lives

Code, scene files, references and ADRs are in git and travel with it. Everything
else that matters — the skills, the memories, the hooks, `slangc`, the systemd
unit and this baseline — is in the migration bundle on `/mnt/data`, because none
of it is in the repository and all of it lives on the drive a reinstall erases.
