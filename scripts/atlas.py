#!/usr/bin/env python3
"""The scene atlas, served locally, with a button that actually opens a scene.

    python3 scripts/atlas.py          # then open http://127.0.0.1:8787

**Why this exists as a local server rather than as the published artifact.**
The artifact on claude.ai is a sandboxed page under a strict CSP: it cannot
reach this machine, and a browser page cannot execute a local command in any
case. So it can only ever *show* you the command. Serving the same page from
here means the button and the engine are on the same host, and a click can
spawn `loom run`.

**What it will and will not run.** The scene list is parsed out of `GOLDEN` in
`xtask/src/main.rs` at startup, and `/run` accepts a *name from that list* and
nothing else — never a path, never arguments, never a command. That is
deliberate: this binds an HTTP endpoint to process spawning, and the only safe
shape for that is a fixed allowlist decided before the socket opens. It also
binds to 127.0.0.1 rather than 0.0.0.0, so nothing off this machine can reach
it.

Thumbnails are rendered once into `target/atlas/` at each scene's own gate
arguments and cached; the first run therefore takes a few minutes and every
later one is instant. `--refresh` re-renders them.
"""

from __future__ import annotations

import base64
import json
import re
import shutil
import subprocess
import sys
from http.server import BaseHTTPRequestHandler, HTTPServer
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
LOOM = ROOT / "target" / "release" / "loom"
CACHE = ROOT / "target" / "atlas"
PORT = 8787


def golden() -> list[dict]:
    """Every row of `GOLDEN`, with the reasoning the table keeps beside it.

    Parsed rather than duplicated, so the atlas cannot drift from the gate. The
    trailing-comma and multi-line forms both appear in that table — hand-written
    parsers here have missed `proving_ground` for exactly that reason.
    """
    src = (ROOT / "xtask" / "src" / "main.rs").read_text()
    start = src.index("const GOLDEN:")
    body = src[start : src.index("\nconst GOLDEN_SIZE", start)]
    body = body[body.index("= [") + 3 :]

    rows: list[dict] = []
    comment: list[str] = []
    buf = ""
    for line in body.split("\n"):
        stripped = line.strip()
        if stripped.startswith("//"):
            if not buf:
                comment.append(stripped.lstrip("/ ").rstrip())
            continue
        buf += " " + stripped
        if buf.count("(") and buf.count("(") == buf.count(")"):
            m = re.search(
                r'\(\s*"([^"]+)"\s*,\s*"([^"]+)"\s*,\s*&\[(.*?)\]\s*,?\s*\)', buf, re.S
            )
            if m:
                rows.append(
                    {
                        "name": m.group(1),
                        "path": m.group(2),
                        "args": re.findall(r'"([^"]*)"', m.group(3)),
                        "note": re.sub(r"\s+", " ", " ".join(comment)).strip(),
                    }
                )
                comment = []
            buf = ""
    return rows


def thumbnails(rows: list[dict], refresh: bool) -> None:
    """Render each scene once, at its own gate arguments, and cache the result."""
    CACHE.mkdir(parents=True, exist_ok=True)
    magick = shutil.which("magick")
    todo = [r for r in rows if refresh or not (CACHE / f"{r['name']}.jpg").exists()]
    if not todo:
        return
    print(f"rendering {len(todo)} thumbnail(s) — first run only", file=sys.stderr)
    for i, r in enumerate(todo, 1):
        png = CACHE / f"{r['name']}.png"
        print(f"  [{i}/{len(todo)}] {r['name']}", file=sys.stderr)
        subprocess.run(
            [str(LOOM), "render", r["path"], *r["args"], "--size", "480x300",
             "--out", str(png)],
            cwd=ROOT, capture_output=True, check=False,
        )
        if not png.exists():
            continue
        if magick:
            subprocess.run([magick, str(png), "-resize", "420x", "-quality", "72",
                            str(CACHE / f"{r['name']}.jpg")], check=False)
            png.unlink(missing_ok=True)
        else:
            png.rename(CACHE / f"{r['name']}.jpg")


def embed(rows: list[dict]) -> list[dict]:
    out = []
    for r in rows:
        f = CACHE / f"{r['name']}.jpg"
        img = ""
        if f.exists():
            kind = "jpeg" if f.read_bytes()[:2] == b"\xff\xd8" else "png"
            img = f"data:image/{kind};base64," + base64.b64encode(f.read_bytes()).decode()
        out.append({**r, "img": img,
                    "cmd": f"loom run {r['path']}"})
    return out


PAGE = """<!doctype html><html><head><meta charset="utf-8">
<title>Loom Scene Atlas — local</title>
<link rel="preconnect" href="https://fonts.googleapis.com">
<link rel="stylesheet" href="https://fonts.googleapis.com/css2?family=Instrument+Serif:ital@0;1&family=IBM+Plex+Mono:wght@400;500;600&family=IBM+Plex+Sans:wght@400;500&display=swap">
<style>
:root{--ground:#0e1417;--surface:#161e22;--surface-2:#1d272c;--line:#2a373d;
 --ink:#dee6e7;--ink-2:#a8b6ba;--muted:#78888d;--accent:#e2a355;--accent-ink:#17202a;--cool:#7fb0c4;--go:#7fbf8a}
@media (prefers-color-scheme:light){:root:not([data-theme="dark"]){
 --ground:#f3f1ec;--surface:#fff;--surface-2:#eae7e0;--line:#d8d3ca;
 --ink:#1b2327;--ink-2:#4a565c;--muted:#7b878d;--accent:#b26a12;--accent-ink:#fff;--cool:#3d6f85;--go:#2f7a45}}
*{box-sizing:border-box}
body{margin:0;background:var(--ground);color:var(--ink);font-family:"IBM Plex Sans",system-ui,sans-serif;font-size:15px;line-height:1.55}
.wrap{max-width:1400px;margin:0 auto;padding:0 24px 72px}
header{padding:48px 0 24px;border-bottom:1px solid var(--line);margin-bottom:20px}
.eyebrow{font-family:"IBM Plex Mono",monospace;font-size:11px;letter-spacing:.16em;text-transform:uppercase;color:var(--muted);margin:0 0 12px}
h1{font-family:"Instrument Serif",Georgia,serif;font-weight:400;font-size:clamp(36px,5vw,58px);line-height:1.03;margin:0 0 12px;text-wrap:balance}
h1 em{font-style:italic;color:var(--accent)}
.lede{max-width:64ch;color:var(--ink-2);margin:0}
.controls{position:sticky;top:0;z-index:20;background:var(--ground);padding:14px 0 12px;border-bottom:1px solid var(--line);margin-bottom:22px;display:flex;flex-wrap:wrap;gap:10px}
#q{flex:1 1 240px;padding:9px 13px;border:1px solid var(--line);border-radius:2px;background:var(--surface);color:var(--ink);font-family:"IBM Plex Mono",monospace;font-size:13px}
#q:focus{outline:2px solid var(--accent);outline-offset:1px}
.grid{display:grid;grid-template-columns:repeat(auto-fill,minmax(268px,1fr));gap:16px}
.card{border:1px solid var(--line);background:var(--surface);border-radius:3px;overflow:hidden;display:flex;flex-direction:column}
.card img{width:100%;aspect-ratio:8/5;object-fit:cover;display:block;background:var(--surface-2);cursor:zoom-in}
.meta{padding:10px 12px 12px;display:flex;flex-direction:column;gap:8px}
.nm{font-family:"IBM Plex Mono",monospace;font-size:13px;font-weight:500;word-break:break-all}
.row{display:flex;gap:8px;align-items:center;justify-content:space-between}
.args{font-family:"IBM Plex Mono",monospace;font-size:11px;color:var(--muted)}
button.go{font-family:"IBM Plex Mono",monospace;font-size:11.5px;letter-spacing:.06em;text-transform:uppercase;
 border:1px solid var(--go);background:transparent;color:var(--go);border-radius:2px;padding:5px 12px;cursor:pointer}
button.go:hover{background:var(--go);color:var(--ground)}
button.go:disabled{opacity:.5;cursor:default}
button.go:focus-visible{outline:2px solid var(--accent);outline-offset:2px}
dialog{border:1px solid var(--line);background:var(--surface);color:var(--ink);border-radius:4px;padding:0;max-width:min(920px,94vw);width:100%}
dialog::backdrop{background:rgba(6,10,12,.7)}
.d{padding:20px 24px 24px}
.d h2{font-family:"IBM Plex Mono",monospace;font-size:18px;margin:0 0 12px}
dialog img{width:100%;border:1px solid var(--line);border-radius:2px;margin-bottom:14px}
.note{max-width:72ch;color:var(--ink-2);font-size:14.5px}
.note strong{color:var(--ink)}.note code{font-family:"IBM Plex Mono",monospace;color:var(--accent);font-size:.9em}
#toast{position:fixed;left:50%;transform:translateX(-50%);bottom:26px;background:var(--surface-2);border:1px solid var(--line);
 color:var(--ink);font-family:"IBM Plex Mono",monospace;font-size:12.5px;padding:9px 16px;border-radius:2px;opacity:0;transition:opacity .18s;pointer-events:none}
#toast.on{opacity:1}
@media (prefers-reduced-motion:reduce){*{transition:none!important}}
</style></head><body>
<div class="wrap">
<header>
  <p class="eyebrow">Loom · served from this machine · <span id="count"></span> scenes</p>
  <h1>Click <em>Open</em> and the engine actually starts.</h1>
  <p class="lede">The same golden reference set, served locally so a button can reach the binary. Open launches <code>loom run</code> on this machine; the thumbnail is the frame the gate compares.</p>
</header>
<div class="controls"><input id="q" type="search" placeholder="Search scenes, paths, reasoning…" aria-label="Search"></div>
<div class="grid" id="grid"></div>
</div>
<dialog id="dlg"><div class="d"><h2 id="dn"></h2><img id="di" alt=""><div class="note" id="dt"></div></div></dialog>
<div id="toast"></div>
<script>
const SCENES = __PAYLOAD__;
const grid=document.getElementById('grid'), q=document.getElementById('q'),
      dlg=document.getElementById('dlg'), toast=document.getElementById('toast');
document.getElementById('count').textContent = SCENES.length;
const esc=s=>String(s).replace(/[&<>"]/g,c=>({'&':'&amp;','<':'&lt;','>':'&gt;','"':'&quot;'}[c]));
const rich=s=>esc(s).replace(/\\*\\*(.+?)\\*\\*/g,'<strong>$1</strong>').replace(/`([^`]+)`/g,'<code>$1</code>');
function say(m){toast.textContent=m;toast.classList.add('on');setTimeout(()=>toast.classList.remove('on'),2600);}
function render(){
  const t=q.value.trim().toLowerCase();
  const hits=SCENES.filter(s=>!t||(s.name+' '+s.path+' '+s.note).toLowerCase().includes(t));
  grid.innerHTML=hits.map(s=>`<div class="card">
    <img src="${s.img}" alt="${esc(s.name)}" loading="lazy" data-i="${SCENES.indexOf(s)}">
    <div class="meta"><span class="nm">${esc(s.name)}</span>
    <span class="row"><button class="go" data-run="${esc(s.name)}">Open</button>
    <span class="args">${esc(s.args.join(' '))||'—'}</span></span></div></div>`).join('');
}
grid.addEventListener('click',async e=>{
  const img=e.target.closest('img[data-i]');
  if(img){const s=SCENES[+img.dataset.i];
    document.getElementById('dn').textContent=s.name;
    document.getElementById('di').src=s.img;
    document.getElementById('dt').innerHTML=s.note?rich(s.note):'<em>No note in the gate table.</em>';
    dlg.showModal();return;}
  const b=e.target.closest('button[data-run]');
  if(!b)return;
  b.disabled=true;const was=b.textContent;b.textContent='Opening';
  try{
    const r=await fetch('/run',{method:'POST',headers:{'Content-Type':'application/json'},
                               body:JSON.stringify({name:b.dataset.run})});
    const j=await r.json();
    say(j.ok?`${j.name} — window opening (pid ${j.pid})`:`could not open: ${j.error}`);
  }catch(err){say('the atlas server is not running');}
  b.disabled=false;b.textContent=was;
});
dlg.addEventListener('click',e=>{if(e.target===dlg)dlg.close();});
q.addEventListener('input',render);
render();
</script></body></html>"""


class Handler(BaseHTTPRequestHandler):
    scenes: dict[str, dict] = {}
    page: bytes = b""

    def log_message(self, *_args):  # noqa: D102 - quiet by default
        pass

    def _send(self, code: int, body: bytes, kind: str) -> None:
        self.send_response(code)
        self.send_header("Content-Type", kind)
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def do_GET(self):  # noqa: N802
        if self.path in ("/", "/index.html"):
            self._send(200, self.page, "text/html; charset=utf-8")
        else:
            self._send(404, b"not found", "text/plain")

    def do_POST(self):  # noqa: N802
        if self.path != "/run":
            self._send(404, b'{"ok":false,"error":"no such endpoint"}', "application/json")
            return
        length = int(self.headers.get("Content-Length") or 0)
        try:
            name = json.loads(self.rfile.read(length) or b"{}").get("name", "")
        except json.JSONDecodeError:
            name = ""
        row = self.scenes.get(name)
        # **The allowlist is the whole security model.** A name from `GOLDEN`,
        # resolved to a path decided before the socket opened — never a path or
        # an argument off the wire.
        if row is None:
            self._send(400, json.dumps({"ok": False, "error": f"unknown scene {name!r}"}).encode(),
                       "application/json")
            return
        proc = subprocess.Popen(  # noqa: S603 - fixed binary, allowlisted argument
            [str(LOOM), "run", row["path"]],
            cwd=ROOT, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL,
            start_new_session=True,
        )
        self._send(200, json.dumps({"ok": True, "name": name, "pid": proc.pid}).encode(),
                   "application/json")


def main() -> int:
    if not LOOM.exists():
        print(f"no engine at {LOOM} — run `cargo build --release` first", file=sys.stderr)
        return 2
    rows = golden()
    thumbnails(rows, refresh="--refresh" in sys.argv)
    Handler.scenes = {r["name"]: r for r in rows}
    Handler.page = PAGE.replace("__PAYLOAD__", json.dumps(embed(rows))).encode()
    server = HTTPServer(("127.0.0.1", PORT), Handler)
    print(f"atlas: {len(rows)} scenes at http://127.0.0.1:{PORT}  (ctrl-c to stop)")
    try:
        server.serve_forever()
    except KeyboardInterrupt:
        print("\natlas: stopped")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
