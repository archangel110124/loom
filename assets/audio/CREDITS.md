# Audio credits and licences

Every clip in this directory, where it came from, and what its licence permits.
**A clip whose licence cannot be established does not go in here.**

| file | origin | licence | attribution required |
| --- | --- | --- | --- |
| `sea_wash.wav` | Sonniss GDC 2019 Game Audio Bundle | Sonniss #GameAudioGDC bundle licence | no |
| `hull_wash.wav` | freesound.org 826442 by RealSquink | CC0 1.0 | no |
| `engine_idle.wav` | Sonniss GDC 2019 Game Audio Bundle | Sonniss #GameAudioGDC bundle licence | no |
| `rain.wav` | see the rain work | — | — |
| `sea.wav`, `hum.wav` | synthesised in this repository | n/a | n/a |

Nothing here was produced by a generative audio model. That is a deliberate
choice and the evidence for it is in the last section.

---

## `sea_wash.wav` — the hub's sea bed

- **plays at** `deeper_demo.loom` node `Rig/SeaWash`, range 55, volume 0.70
- **title** — *ambient Norway … Soft Waves Cliffs*
- **source** — `Sonniss.com - GDC 2019 - Game Audio Bundle Part 6of8` /
  `WATRWave_Soft Waves Cliffs_JSE_RCoN_Stereo.wav`, a 24-bit / 96 kHz stereo
  production master
- **url** — <https://downloads.sonniss.com/Sonniss.com%20-%20GDC%202019%20-%20Game%20Audio%20Bundle%20Part%206of8.zip>
- **licence** — Sonniss #GameAudioGDC Bundle Licence, verified at
  <https://sonniss.com/gdc-bundle-license/> on 2026-08-20: *"worldwide,
  non-exclusive, royalty-free … unlimited number of projects for the entirety of
  their life time … without attribution … includes but is not limited to:
  games."*
- **restrictions** — may not be resold as-is; may not be modified so as to claim
  authorship of the original recording; **may not be used to train AI models.**
- **cut** — channel L (1.69× the high-frequency energy of the L+R sum: Loom
  folds a positioned stereo clip by averaging, and this near-coincident pair
  combs when summed); 30 Hz high-pass; 10.2 kHz low-pass; resampled 96000 →
  22050; **start 151.0 s, length 40 s**, 1.5 s equal-power crossfade; −20 LUFS.
- **why that window** — the parent is 243 s and holds 204 candidate 40 s starts.
  Their level swing runs 5.63 – 10.93 dB with a median of 8.63. This window is
  8.45 dB: a sea that breathes as much as the recording does on average. An
  earlier cut at 65.0 s measured 6.54 dB — the **11th percentile**, near the
  flattest stretch in the file — because the window selector sorted by *lowest*
  spread. Flat is safe and reads as hiss.
- **loop** — seam sample step 0.00354 through Loom's own decoder, against a
  median interior step of 0.01831: the splice is a fifth of an ordinary sample
  step. Block level step across the loop point 0.12 dB against 2.09 dB at the
  95th percentile of interior points. The loop point's own quietest moment sits
  at the **57th percentile** of the file's lulls — an unremarkable instant, not
  a hole.

## `hull_wash.wav` — water on the hull

- **plays at** `deeper_demo.loom` node `Rig/Boat/Wash`, range 35, volume 0.50
- **title** — *Ocean Pier – Sloshing Water, Salt Spray*
- **source** — freesound.org sound 826442 by **RealSquink**
- **url** — <https://freesound.org/people/RealSquink/sounds/826442/>
- **licence** — **CC0 1.0 Universal** (public domain dedication),
  <http://creativecommons.org/publicdomain/zero/1.0/>, verified on the sound's
  own page on 2026-08-20. No attribution required; RealSquink is credited here
  as a courtesy.
- **master** — freesound's public high-quality preview, mp3 185 kbps 48 kHz
  stereo, not the lossless original (which is behind a login). At a 22050 Hz
  output the codec's own band limit is far above anything that survives, so the
  preview is not the quality ceiling here.
- **cut** — channel L (1.33× the sum); 40 Hz high-pass; 10.2 kHz low-pass;
  48000 → 22050; start 110.0 s, length 24 s, 1.0 s crossfade; −21.5 LUFS.
- **why a second recording at all** — `Voice::new` hardcodes `cursor: 0.0`
  (`crates/loom_audio/src/mix.rs:55`) and takes no offset, so two `AudioSource`s
  pointing at one clip start on the same frame and never drift apart. They
  reinforce instead of layering. Two clips is the only fix that does not need an
  engine change.

## `engine_idle.wav` — the boat's engine

- **plays at** `deeper_demo.loom` node `Rig/Boat/Engine`, range 14, volume 0.85,
  with `engine_note.rhai` moving it vertically against `state.knots`
- **title** — *tugboat Deutz VM 145, on-board idle, steady, deck, propeller*
- **source** — `Sonniss.com - GDC 2019 - Game Audio Bundle Part 5of8` /
  Pole Position — Tugboat Deutz VM 145 /
  `tugboat_t24_onbrd_idle_steady_deck_propeller_MKH8040.wav`, 24-bit / 96 kHz
- **url** — <https://downloads.sonniss.com/Sonniss.com%20-%20GDC%202019%20-%20Game%20Audio%20Bundle%20Part%205of8.zip>
- **licence** — Sonniss #GameAudioGDC Bundle Licence, as above. No attribution
  required. Same restrictions, including the AI-training prohibition.
- **cut** — channel L (1.71× the sum); 25 Hz high-pass; 10.2 kHz low-pass;
  96000 → 22050; start 24.4 s, length 8 s, 0.10 s crossfade; −20 LUFS.
- **not period-aligned, and that was measured** — sliding the loop end to the
  best waveform match scored *worse* (seam click ratio 0.88 aligned against 0.48
  not), because the best correlation available was only 0.564. This recording is
  not periodic enough for alignment to pay.
- **what it replaces** — `hum.wav`, which is three sine partials: black above
  400 Hz on a spectrogram and flat to within 0.2 dB on a level plot. `hum.wav`
  is still used by `assets/test/range.loom` and `assets/games/proving_ground.loom`,
  where a pure tone is a fine test signal.

---

## Why nothing here was generated

Stable Audio Open was set up locally and its output was put on the same
instruments as the downloads. It was rejected on a measurement, not a hunch.

The four loudest narrowband peaks in each file, as dB over the local median
across frequency, with the fraction of frames each is present in:

| file | peak | prominence | presence |
| --- | --- | --- | --- |
| real: *Soft Waves Cliffs* parent | 234 Hz | 1.2 dB | 21% |
| real: *Ocean Pier* parent | 8508 Hz | 0.9 dB | 14% |
| shipped: `sea_wash.wav` | 202 Hz | 1.8 dB | 17% |
| shipped: `rain.wav` (accepted by the human) | 377 Hz | 2.8 dB | 24% |
| generated: sea candidate A | 1744 Hz | 8.7 dB | 91% |
| generated: sea candidate B | 1744 Hz | 9.2 dB | 96% |

Same frequency across two seeds, surviving the spectral flattening applied to
try to remove it, and continuous through the passages where the water itself
drops away. That is a fixed whistle sitting on top of the sea.

The same test rejected a downloaded clip too — a Wellington Pier metal-rustling
recording measured 9.9 dB at 4530 Hz in 74% of frames, worse than the generated
audio, and it is in the parent recording so it could not be cut around. The rule
is applied to downloads and to generated audio alike.
