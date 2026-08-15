#!/bin/bash
cd /home/k-dorui/loom/.claude/worktrees/loom-companion-docs
for n in 0 2 4 6 10 14; do
  sed -i "s/^static const int EXTRA_TRIPLES = .*/static const int EXTRA_TRIPLES = $n;/" assets/shaders/scene.slang
  cargo build --release -p loom_cli >/dev/null 2>&1
  med=$(LOOM_GPU_TIMING=1 ./target/release/loom render assets/test/terrain_stress.loom --out /tmp/ts.png --size 1920x1080 --frames 11 2>&1 | grep -o 'forward [0-9.]*' | awk '{print $2}' | sort -n | awk '{a[NR]=$1} END{print a[int(NR/2)+1]}')
  echo "EXTRA_TRIPLES=$n  samples_per_ground_px=$((7 + n * 3))  forward=${med} ms"
done
