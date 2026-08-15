#!/bin/bash
cd /home/k-dorui/loom/.claude/worktrees/loom-companion-docs
for n in 0 2 4 6 10 14 0; do
  sed -i "s/^static const int EXTRA_TRIPLES = .*/static const int EXTRA_TRIPLES = $n;/" assets/shaders/scene.slang
  cargo build --release -p loom_cli >/dev/null 2>&1
  # warm-up render, discarded: the 4090 idles at low clocks and the first
  # render of a process ramps them.
  LOOM_GPU_TIMING=1 ./target/release/loom render assets/test/terrain_stress.loom --out /tmp/ts.png --size 1920x1080 --frames 60 >/dev/null 2>&1
  vals=$(LOOM_GPU_TIMING=1 ./target/release/loom render assets/test/terrain_stress.loom --out /tmp/ts.png --size 1920x1080 --frames 60 2>&1 | grep -o 'forward [0-9.]*' | awk '{print $2}' | sort -n)
  min=$(echo "$vals" | head -1)
  med=$(echo "$vals" | awk '{a[NR]=$1} END{print a[int(NR/2)+1]}')
  echo "extra_triples=$n  samples_per_ground_px=$((7 + n * 3))  min=${min} ms  median=${med} ms"
done
