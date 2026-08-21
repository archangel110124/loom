#!/usr/bin/env python3
"""THE DECKHAND — emits the prefab and the round-1 proof scenes.

    python3 emit_deckhand.py <worktree>

Writes, and nothing else:
    <worktree>/assets/prefabs/deckhand.loom          the 43-node prefab, at rest
    r1/scenes/view_{front,side,threeq,back}.loom     via `prefab = "deckhand"`
    r1/scenes/sil_{side,front}.loom                  black albedo, white sky
    r1/scenes/pose_{00..07}.loom                     one stride, inlined
    r1/scenes/pose_hold.loom                         the rod-at-ready pose

TWO CONSTRUCTION RULES, both from fishing_rod.loom's own header and both
enforced by `piv`/`leaf` below being separate functions:

  * A PIVOT carries rotation only, at scale [1,1,1], ALWAYS.  compose()
    (loom_ecs/src/lib.rs:873) scales the local matrix's COLUMNS, so a parent's
    non-uniform scale lands after a child's rotation and shears it.
  * A LEAF carries the scale and has no children.

EVERY SCRIPTED ROTATION CHANNEL IS AUTHORED AT ZERO.  A node Script has no
memory and `play.rs:2483` reads the node's LIVE transform, so a shared script
must write absolute values -- and there is no rest transform to add them to.
Identity comes from `position[0]`, the one channel nothing writes.
"""
import math
import os
import sys
import tempfile
import uuid

DIR = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, DIR)
import deckhand_spec as S  # noqa: E402

PREFAB_ID = "7a41c0de-5b2e-4f18-9d63-2c8ae5f10b47"   # = the prefab file's [scene] id
# The prefab is the tracked output.  The proof scenes below it are a
# round's scratch -- they go to a temp directory unless `DECKHAND_SCENES`
# names one, so running this from a clean checkout writes exactly one
# tracked file.
SCENES = os.environ.get("DECKHAND_SCENES",
                        tempfile.mkdtemp(prefix="deckhand-scenes-"))


def aid(key):
    return str(uuid.uuid5(uuid.UUID(PREFAB_ID), key))


def piv(name, parent, pos, rot=(0.0, 0.0, 0.0), script=None):
    p = f'parent = "{parent}"\n' if parent else ""
    s = "" if script is None else \
        f'\n  [node.components.Script]\n  path = "../scripts/{script}.rhai"\n'
    return (f'\n[[node]]\nname = "{name}"\n{p}'
            f'transform = {{ pos = [{pos[0]:.4f}, {pos[1]:.4f}, {pos[2]:.4f}], '
            f'rot_euler = [{rot[0]:.3f}, {rot[1]:.3f}, {rot[2]:.3f}] }}\n{s}')


# THE ANIMATION, and it is attached HERE rather than on an instance because
# `prefab::expand` keeps an instance node's `transform` and its
# `[node.overrides]` and DISCARDS its `components` -- so a `Script` written on
# `Rig/Player/Body` is silently dropped and the puppet stands still while
# validating clean.  Same reason `rod_hold.rhai` lives inside
# `fishing_rod.loom`.
#
# Seven files for thirteen joints: one per joint KIND, telling itself apart by
# `sign(position[0])`.  `deckhand_hips.rhai` is the exception and is the only
# node that uses it, which is what lets it hardcode its own base of 0.800.
#
# A scene with no `state.gait` reads nothing: every one of them defaults the
# blend to 0 and writes the authored rest pose back unchanged, so a deckhand in
# a scene with no rules script simply stands there.
SCRIPTS = {
    "Hips": "deckhand_hips",
    "Spine": "deckhand_spine",
    "Neck": "deckhand_neck",
    "HipL": "deckhand_hip", "HipR": "deckhand_hip",
    "KneeL": "deckhand_knee", "KneeR": "deckhand_knee",
    "AnkleL": "deckhand_ankle", "AnkleR": "deckhand_ankle",
    "ShoulderL": "deckhand_shoulder", "ShoulderR": "deckhand_shoulder",
    "ElbowL": "deckhand_elbow", "ElbowR": "deckhand_elbow",
}


def leaf(name, parent, pos, asset, mat, scale=None, rot=(0.0, 0.0, 0.0)):
    a, m, r, po = mat
    sc = "" if scale is None else \
        f', scale = [{scale[0]:.4f}, {scale[1]:.4f}, {scale[2]:.4f}]'
    return (f'\n[[node]]\nname = "{name}"\nparent = "{parent}"\n'
            f'transform = {{ pos = [{pos[0]:.4f}, {pos[1]:.4f}, {pos[2]:.4f}], '
            f'rot_euler = [{rot[0]:.2f}, {rot[1]:.2f}, {rot[2]:.2f}]{sc} }}\n'
            f'\n  [node.components.MeshRenderer]\n  mesh = {{ asset = "{asset}" }}\n'
            f'\n  [node.components.Material]\n'
            f'  albedo = [{a[0]:.3f}, {a[1]:.3f}, {a[2]:.3f}]\n'
            f'  metallic = {m}\n  roughness = {r}\n  porosity = {po}\n')


def assets(meshdir):
    o = []
    for k in S.MESHES:
        o.append(f'\n[[asset]]\nkey = "{k}"\nid = "{aid(k)}"\n'
                 f'path = "{meshdir}/deckhand_{k}.obj"\n')
    return "".join(o)


# ------------------------------------------------------------------- body
def body(root, parent, p=None, mats=None, scripts=False):
    """The 44 nodes.  `p` is a pose dict; None means rest (all zero).

    `scripts=True` hangs SCRIPTS on the sixteen pivots.  Only the prefab
    asks for it: every baked-pose proof scene wants the pose the file
    authors and nothing writing over it on tick 1.
    """
    p = dict(S.REST) if p is None else p
    def sc(name):
        return SCRIPTS.get(name) if scripts else None
    M = mats or {}

    def mat(name, default):
        return M.get(name, default)

    OIL, WADER, SOU = mat("oil", S.OIL), mat("wader", S.WADER), mat("sou", S.SOU)
    BOOT, HAND, BRASS = mat("boot", S.BOOT), mat("hand", S.HAND), mat("brass", S.BRASS)
    LENS, WICKER = mat("lens", S.LENS), mat("wicker", S.WICKER)
    SKIN = mat("skin", S.SKIN)

    o = [piv(root, parent, (0.0, 0.0, 0.0))]
    R = f"{parent}/{root}" if parent else root

    o.append(piv("Hips", R, (0.0, S.CROTCH + p["bob"], 0.0), (0.0, 0.0, p["roll"]),
                 sc("Hips")))
    HI = f"{R}/Hips"
    o.append(leaf("HipsM", HI, (0.0, 0.123, 0.0), "pelvis", WADER))

    o.append(piv("Spine", HI, (0.0, S.PELVIS, 0.0), (p["lean"], p["yaw"], 0.0),
                 sc("Spine")))
    SP = f"{HI}/Spine"
    o.append(leaf("ChestM", SP, (0.0, 0.197, 0.0), "torso", OIL))
    o.append(leaf("CreelM", SP, (0.0, 0.181, S.CREEL_Z), "creel", WICKER))
    # The collar's height is DERIVED from NECK_GAP, not authored, so the
    # 34 mm of bare neck cannot be closed by moving either the hat or the ring.
    o.append(leaf("CollarM", SP, (0.0, S.COLLAR_WY - (S.NECK_Y - S.CHEST), 0.0),
                  "cylinder", BRASS, (S.COLLAR_R, S.COLLAR_H, S.COLLAR_R)))

    o.append(piv("Neck", SP, (0.0, S.CHEST, 0.0), (0.0, p["nyaw"], 0.0),
                 sc("Neck")))
    NK = f"{SP}/Neck"
    # 34 mm OF VISIBLE NECK, which is neither of the two previous answers.
    # Round 1 left 101 mm of bare stalk (a hat on a stick); round 2 closed it
    # to a 16 mm overlap and the head fused into the shoulders in every view.
    # The gusset runs from the collar up INTO the hat's opening, so the gap
    # shows dark neck rather than a hole, and it is the gap -- not the brass
    # ring -- that separates head-mass from body-mass in the outline.
    half = (S.NECK_TOP - S.NECK_Y) * 0.5
    o.append(leaf("NeckM", NK, (0.0, half, 0.0), "cylinder", OIL,
                  (S.NECK_R, half, S.NECK_R)))
    # The plug that stops the hat being an empty bell -- see the block
    # around HEAD_R in deckhand_spec.py.  The engine's own `cylinder`,
    # so no ninth OBJ, and 256 triangles.
    o.append(leaf("HeadM", NK, (0.0, S.HEAD_Y, 0.0), "cylinder", SKIN,
                  (S.HEAD_R, S.HEAD_H, S.HEAD_R)))
    o.append(leaf("HatM", NK, (0.0, S.HAT_Y, S.HAT_Z), "hat", SOU))
    # LampM: on the Spine (a CHEST lamp), and now FLATTENED INTO THE COAT and
    # moved off-centre.  Round 1 put it on the head, where it sat 180 mm in
    # front of the eye and filled the lower third of the first-person frame;
    # moving it to the chest fixed that and introduced a different one.  As a
    # tube standing 48 mm proud of a centred chest it read as a spigot, and it
    # was worse in motion than in the still -- it is the second-brightest
    # thing on him and it NEVER MOVES relative to the chest while every other
    # part does, so the eye locks onto it and it competes directly with the
    # hat-as-beacon plan.  Set into the oilskin at the left breast it is a
    # lamp on a coat: still the bright dot a teammate sees at twelve metres,
    # no longer a rod sticking out of a man's sternum.
    # ROUND 3 SHRINKS IT AGAIN, 43 mm radius to 26.  At 86 mm across it was a
    # white DISC on the chest and read as a name badge, not a lamp -- and it is
    # the second-brightest value on a figure whose whole legibility plan is
    # "one bright feature", so it was spending a scarce value on nothing.
    o.append(leaf("LampM", SP, (-0.082, 0.238, S.CHEST_CZ - S.CHEST_HD * 0.93),
                  "cylinder", LENS, (0.026, 0.013, 0.026), (90.0, 0.0, 0.0)))

    # EVERY PIVOT IN A LIMB CHAIN CARRIES SIDE_EPS OF LOCAL X, and it is not
    # decoration.  The animation contract identifies a joint's side from
    # sign(position[0]) -- the one channel no script writes -- but round 1
    # authored the elbow, knee, ankle and hand pivots at x = 0.0 on BOTH
    # sides, so `if position[0] < 0.0` was false on both and six of the
    # fourteen joints could not be told apart at all.  Knee and elbow got away
    # with it because they are rectified and |sin(p)| == |sin(p+pi)|; the
    # ankle is NOT rectified, so both feet rolled in unison while the legs
    # were half a cycle apart.  One millimetre is invisible and makes the
    # contract uniform rather than accidentally-correct at four joints of six.
    E = S.SIDE_EPS
    for s, sx, sh, el in (("L", -1.0, p["shL"], p["elL"]),
                          ("R", +1.0, p["shR"], p["elR"])):
        # rot_euler [swing, 0, splay].  Y-X-Z means the SPLAY APPLIES FIRST,
        # so it tucks the elbow inboard without tilting the swing plane -- it
        # is what turns two independent arm swings into a two-handed grip.
        # Authored at zero; only the hold pose uses it.
        o.append(piv(f"Shoulder{s}", SP, (sx * S.SH_X, S.CHEST - S.SH_DROP, 0.0),
                     (sh, 0.0, p[f"sp{s}"]), sc(f"Shoulder{s}")))
        SH = f"{SP}/Shoulder{s}"
        o.append(leaf(f"ShoulderBall{s}", SH, (0, 0, 0), "ball", OIL,
                      (S.BALL_SHOULDER,) * 3))
        o.append(leaf(f"UpperArm{s}M", SH, (0.0, -S.UPPER * 0.5, 0.0), "limb", OIL,
                      S.limb_scale(S.UPPER, S.ARM_R)))
        o.append(piv(f"Elbow{s}", SH, (sx * E, -S.UPPER, 0.0), (el, 0.0, 0.0),
                     sc(f"Elbow{s}")))
        EL = f"{SH}/Elbow{s}"
        o.append(leaf(f"ElbowBall{s}", EL, (0, 0, 0), "ball", OIL,
                      (S.BALL_ELBOW,) * 3))
        o.append(leaf(f"Forearm{s}M", EL, (0.0, -S.FORE * 0.5, 0.0), "limb", OIL,
                      S.limb_scale(S.FORE, S.FORE_R)))
        o.append(piv(f"Hand{s}", EL, (sx * E, -S.FORE, 0.0),
                     (p.get(f"wr{s}", 0.0), 0.0, 0.0)))
        HN = f"{EL}/Hand{s}"
        o.append(leaf(f"Hand{s}M", HN, (0.0, -0.066, 0.0), "hand", HAND))

    for s, sx, hp, kn, an in (("L", -1.0, p["hipL"], p["kneeL"], p["ankL"]),
                              ("R", +1.0, p["hipR"], p["kneeR"], p["ankR"])):
        o.append(piv(f"Hip{s}", HI, (sx * S.HIP_X, 0.0, 0.0), (hp, 0.0, 0.0),
                     sc(f"Hip{s}")))
        HP = f"{HI}/Hip{s}"
        o.append(leaf(f"HipBall{s}", HP, (0, 0, 0), "ball", WADER,
                      (S.BALL_HIP,) * 3))
        o.append(leaf(f"Thigh{s}M", HP, (0.0, -S.THIGH * 0.5, 0.0), "limb", WADER,
                      S.limb_scale(S.THIGH, S.LEG_R)))
        o.append(piv(f"Knee{s}", HP, (sx * E, -S.THIGH, 0.0), (kn, 0.0, 0.0),
                     sc(f"Knee{s}")))
        KN = f"{HP}/Knee{s}"
        o.append(leaf(f"KneeBall{s}", KN, (0, 0, 0), "ball", WADER,
                      (S.BALL_KNEE,) * 3))
        o.append(leaf(f"Shin{s}M", KN, (0.0, -S.SHIN * 0.5, 0.0), "limb", WADER,
                      S.limb_scale(S.SHIN, S.SHIN_R)))
        o.append(piv(f"Ankle{s}", KN, (sx * E, -S.SHIN, 0.0), (an, 0.0, 0.0),
                     sc(f"Ankle{s}")))
        AN = f"{KN}/Ankle{s}"
        o.append(leaf(f"Boot{s}M", AN, (0.0, S.BOOT_Y, S.BOOT_Z), "boot", BOOT))

    return "".join(o)


HEADER = '''# THE DECKHAND -- a wooden artist's mannequin in black oilskin and an ochre
# sou'wester.  44 nodes: 16 pivots that rotate and 28 mesh leaves that do not.
#
# Loom has no skeletal animation -- no rig format, no skinning, no bones, no
# import path for any of them (loom_asset/src/mesh.rs:316 iterates
# document.meshes() and never skins() or animations()).  So this character is
# separate rigid parts on a node hierarchy, exactly like fishing_rod.loom's
# 24-segment chain, and a rhai script bends it by writing rotations.  That is
# not a compromise: ADR 0062:85 says every ray -- reflection, shadow, RTAO --
# sees a Deform'd or skinned mesh at its REST pose, so an articulated puppet is
# the only animation method in this engine a mirror can reflect in motion.
#
# TWO RULES.  A PIVOT carries rotation only, at scale [1,1,1], always:
# compose() scales the local matrix's COLUMNS, so a parent's non-uniform scale
# is applied after a child's rotation and shears it.  A LEAF carries the scale
# and has no children.
#
# EVERY SCRIPTED ROTATION CHANNEL IS AUTHORED AT ZERO, and that is load-bearing
# rather than tidy.  A node Script has no memory and play.rs:2483 reads the
# node's LIVE transform, ticks the script and writes the result straight back,
# so a shared script must write ABSOLUTE values and there is no rest pose for
# it to add them to.  Joint identity comes from `position[0]` -- the local
# offset, which nothing writes -- so sign(position[0]) is the side.
#
# Authored forward is -Z (play.rs:3710).  Boot toes point -Z; the sou'wester's
# brim sweeps 30% long toward +Z; the creel rides on +Z.
#
# THIS PREFAB'S ORIGIN IS THE SOLE PLANE -- y 0 is the deck he stands on.  The
# demo's `Player` origin is NOT: deeper_demo.loom:697 says "the capsule's
# origin is its centre, 0.90 above its feet".  So mounting this on a
# CharacterController node with an identity transform floats him 0.90 m in the
# air, and the instance needs
#     transform = { pos = [0.0, -0.90, 0.0] }
#
# AND IT CANNOT BE MOUNTED THERE AT ALL UNTIL play.rs:758 IS FIXED.  A
# MeshRenderer under a CharacterController with no dynamic ancestor is turned
# into a STATIC BOX at its rest pose, so all 27 leaves become invisible
# scenery welded to the spawn point -- the player walks into his own chest.
# The one-line fix (a character_ancestor test beside the existing
# dynamic_ancestor one) is not this file's to make; see the blueprint's 0.
#
# Generated by tools/mesh/build_deckhand.py + tools/mesh/emit_deckhand.py.
# Never edit this file: re-run
#     python3 tools/mesh/emit_deckhand.py
# from the workspace root, which rewrites exactly this file.  The Blender
# half is
#     blender --background --factory-startup --python \
#             tools/mesh/build_deckhand.py
# and it rewrites the eight OBJs in assets/meshes/.

[scene]
format = 1
id = "7a41c0de-5b2e-4f18-9d63-2c8ae5f10b47"
'''

ENV = """[scene]
format = 1
id = "%(id)s"
%(assets)s
[[node]]
name = "Root"

  [node.components.Environment]
  sun_direction = [0.34, 0.68, -0.65]
  sun_strength = %(sun)s
  sun_color = [1.0, 0.95, 0.88]
  ambient = %(amb)s
  sky_zenith = %(zen)s
  sky_horizon = %(hor)s
  fog_density = 0.0
  fog_falloff = 0.05
"""

GROUND = """
[[node]]
name = "Ground"
parent = "Root"
transform = { pos = [0.0, 0.0, 0.0], scale = [14.0, 1.0, 14.0] }

  [node.components.MeshRenderer]
  mesh = { asset = "plane" }

  [node.components.Material]
  albedo = [0.24, 0.25, 0.24]
  roughness = 0.95
"""


def cam(pos, rot, fov=40.0):
    return (f'\n[[node]]\nname = "Camera"\nparent = "Root"\n'
            f'transform = {{ pos = [{pos[0]:.3f}, {pos[1]:.3f}, {pos[2]:.3f}], '
            f'rot_euler = [{rot[0]:.2f}, {rot[1]:.2f}, {rot[2]:.2f}] }}\n'
            f'\n  [node.components.Camera]\n  fov_y_degrees = {fov}\n')


MESHDIR = [""]                # set by main() -- absolute, for scratch scenes


def env(idn, sun=2.0, amb=0.40, zen="[0.26, 0.34, 0.46]",
        hor="[0.52, 0.56, 0.58]", with_assets=True):
    return ENV % dict(id=f"d3c8a5e0-9000-4000-8000-{idn:012d}",
                      sun=sun, amb=amb, zen=zen, hor=hor,
                      assets=assets(MESHDIR[0]) if with_assets else "")


# ------------------------------------------------------------------- views
VIEWS = {                     # name: (camera pos, camera rot)
    "front":  ((0.00, 0.95, -3.60), (0.0, 180.0, 0.0)),
    "side":   ((3.60, 0.95, 0.00),  (0.0, 90.0, 0.0)),
    "back":   ((0.00, 0.95, 3.60),  (0.0, 0.0, 0.0)),
    "threeq": ((2.55, 1.15, -2.55), (-3.0, 135.0, 0.0)),
}

FLAT = ((0.0, 0.0, 0.0), 0.0, 1.0, 0.0)   # a pure-black silhouette material


def write(path, text):
    os.makedirs(os.path.dirname(path), exist_ok=True)
    open(path, "w").write(text)


def main(worktree):
    meshdir = os.path.join(worktree, "assets", "meshes")
    MESHDIR[0] = meshdir
    prefab = os.path.join(worktree, "assets", "prefabs", "deckhand.loom")
    write(prefab, HEADER + assets("../meshes")
          + body("Deckhand", None, scripts=True))
    print("prefab  %s" % prefab)

    ref = (f'\n[[prefab]]\nkey = "deckhand"\nid = "{PREFAB_ID}"\n'
           f'path = "{prefab}"\n')
    inst = '\n[[node]]\nname = "Man"\nparent = "Root"\nprefab = "deckhand"\n'

    for i, (n, (cp, cr)) in enumerate(sorted(VIEWS.items())):
        write(os.path.join(SCENES, f"view_{n}.loom"),
              env(600 + i, with_assets=False) + ref + inst
              + GROUND + cam(cp, cr))

    # Silhouette: black albedo, white sky, no ground, no sun.  A shell whose
    # normals point inward comes back pure black too -- which is why this
    # instrument is only ever read for SHAPE, and `lit_*` below is the one that
    # can see an inverted normal.
    black = dict(oil=FLAT, wader=FLAT, sou=FLAT, boot=FLAT, hand=FLAT,
                 brass=FLAT, lens=FLAT, wicker=FLAT)
    for n, pos, rot, p in (("side", (3.60, 0.95, 0.0), (0.0, 90.0, 0.0), None),
                           ("front", (0.0, 0.95, -3.60), (0.0, 180.0, 0.0), None),
                           ("stride", (3.60, 0.95, 0.0), (0.0, 90.0, 0.0),
                            S.pose(0.75))):
        write(os.path.join(SCENES, f"sil_{n}.loom"),
              env(700, sun=0.0, amb=0.0, zen="[1.0, 1.0, 1.0]",
                  hor="[1.0, 1.0, 1.0]")
              + body("Deckhand", "Root", p, black) + cam(pos, rot))

    # THE CRITIC'S OWN TEST, MADE INTO A RENDER: "mask the hat and the creel
    # out of sil_side.png; what remains must still read as a person."  A
    # silhouette scene lights nothing, so albedo cannot hide a part -- but with
    # ambient 1.0 and a white sky a WHITE part is invisible against the
    # background and a black one is not.  So this is the same figure with its
    # two pieces of furniture deleted from the outline, and it is the only
    # honest way to ask whether the body under them has a shape.
    nofurn = dict(black)
    nofurn["sou"] = nofurn["wicker"] = ((1.0, 1.0, 1.0), 0.0, 1.0, 0.0)
    write(os.path.join(SCENES, "sil_nofurn.loom"),
          env(702, sun=0.0, amb=1.0, zen="[1.0, 1.0, 1.0]",
              hor="[1.0, 1.0, 1.0]")
          + body("Deckhand", "Root", None, nofurn)
          + cam((3.60, 0.95, 0.0), (0.0, 90.0, 0.0)))

    # THE WIDEST POSE IS FOUND, NOT ASSUMED.  The blueprint sets a numeric
    # target on "side, contact" -- the widest frame of the cycle -- and round 1
    # took that to be phase 0.75 and measured there.  Phase 0.75 is only the
    # widest pose for the gait it was chosen against; change the knee curve and
    # the widest frame moves, so a fixed phase silently starts measuring
    # something else.  Twelve silhouettes, and r2/measure.py reports the max.
    for i in range(12):
        write(os.path.join(SCENES, f"silw_%02d.loom" % i),
              env(720 + i, sun=0.0, amb=0.0, zen="[1.0, 1.0, 1.0]",
                  hor="[1.0, 1.0, 1.0]")
              + body("Deckhand", "Root", S.pose(i / 12.0), black)
              + cam((3.60, 0.95, 0.0), (0.0, 90.0, 0.0)))

    # AMBIENT-ONLY: sun off, ambient 1.0, white sky.  A shell whose normals
    # face inward loses BOTH diffuse and ambient and renders pure black, so
    # this is the one render that can see rule 2 being broken.
    write(os.path.join(SCENES, "lit_flat.loom"),
          env(701, sun=0.0, amb=1.0, zen="[1.0, 1.0, 1.0]",
              hor="[1.0, 1.0, 1.0]")
          + body("Deckhand", "Root", None) + cam((2.55, 1.15, -2.55),
                                                 (-3.0, 135.0, 0.0)))

    n = 8
    for i in range(n):
        write(os.path.join(SCENES, f"pose_{i:02d}.loom"),
              env(800 + i) + body("Deckhand", "Root", S.pose(i / n))
              + GROUND + cam((3.60, 0.95, 0.0), (0.0, 90.0, 0.0), 34.0))
        # ...AND THE SAME CYCLE FROM THE FRONT, which round 2 never rendered.
        # The pelvis roll and the shoulder counter-yaw are the two channels
        # that carry "a body catching itself", and BOTH of them move the hat
        # along X -- straight into a side camera.  A cycle judged only from
        # the side is a cycle with two of its four upper-body channels
        # invisible, which is how 5 degrees of roll can be added and measured
        # as 2.4 px of movement.
        write(os.path.join(SCENES, f"posef_{i:02d}.loom"),
              env(870 + i) + body("Deckhand", "Root", S.pose(i / n))
              + GROUND + cam((0.0, 0.95, -3.60), (0.0, 180.0, 0.0), 34.0))
    write(os.path.join(SCENES, "pose_hold.loom"),
          env(810) + body("Deckhand", "Root", S.HOLD)
          + GROUND + cam((2.55, 1.15, -2.55), (-3.0, 135.0, 0.0)))

    # THE ROD ACTUALLY IN HIS HAND, front and side.  This is the pose the
    # character spends most of the game in and round 1 never rendered it with
    # the rod attached at all -- `pose_hold.png` was a rear three-quarter with
    # the far arm hidden behind the torso, so the hand-to-hand relationship,
    # which is the thing articulated puppets get wrong, was unproven.
    #
    # The rod parents to HandR, which is a PIVOT and not a leaf, and that is
    # exactly why HandL/HandR are pivots.  Its blank runs up +Y from its own
    # origin, so the rod's world direction is
    #     Rx(sh) * Rz(splay) * Rx(el + wrist) * Rx(theta) * Rz(phi) * (0,1,0)
    # and BECAUSE THE SPLAY SITS IN THE MIDDLE the answer is not `90 - the sum
    # of the X angles` and cannot be got by adding.  r3/rodangle.py solves the
    # two unknowns for a tip 30 deg above the horizon and canted 8 deg across
    # the body -- [-162.2, 0, 5.8], residual 0.0000 -- and asserts the tip
    # ends up above the eye, which is the thing a nudged number gets wrong.
    rod = (f'\n[[prefab]]\nkey = "rod"\n'
           f'id = "7f2c14a9-63d8-4e51-b0a7-2d9e5c81f430"\n'
           f'path = "{os.path.join(worktree, "assets", "prefabs", "fishing_rod.loom")}"\n')
    rodnode = ('\n[[node]]\nname = "Rod"\n'
               'parent = "Root/Deckhand/Hips/Spine/ShoulderR/ElbowR/HandR"\n'
               'prefab = "rod"\n'
               'transform = { pos = [0.0, -0.075, -0.020], '
               'rot_euler = [-162.2, 0.0, 5.8] }\n')
    for n, (cp, cr) in (("front", ((0.0, 1.05, -3.20), (0.0, 180.0, 0.0))),
                        ("side", ((3.20, 1.05, 0.0), (0.0, 90.0, 0.0))),
                        ("threeq", ((2.30, 1.25, -2.30), (-3.0, 135.0, 0.0)))):
        write(os.path.join(SCENES, f"rod_{n}.loom"),
              env(840 + len(n)) + rod
              + body("Deckhand", "Root", S.HOLD)
              + rodnode + GROUND + cam(cp, cr, 42.0))

    # ...and from his own eye, which is the shot that actually ships.
    write(os.path.join(SCENES, "rod_first.loom"),
          env(849) + rod + body("Deckhand", "Root", S.HOLD)
          + rodnode + GROUND
          + cam((0.0, S.EYE_Y, 0.0), (-8.0, 0.0, 0.0), 75.0))

    # FIRST PERSON.  The eye is at world y 1.650 -- inside the closed hat dome,
    # where BACK-face culling (renderer.rs:4478) makes the head disappear for
    # free.  What this render is really for is the two things that are NOT
    # culled: the brim's underside 12 mm below the eye, and the lamp 54 mm
    # below it and 180 mm forward.
    posts = ""
    for i, (x, z, h) in enumerate(((-0.9, -2.4, 1.1), (1.2, -3.1, 1.6),
                                   (0.2, -5.0, 0.8), (-2.2, -4.2, 1.4))):
        posts += (f'\n[[node]]\nname = "Post{i}"\nparent = "Root"\n'
                  f'transform = {{ pos = [{x}, {h * 0.5}, {z}], '
                  f'scale = [0.09, {h * 0.5}, 0.09] }}\n'
                  f'\n  [node.components.MeshRenderer]\n  mesh = {{ asset = "box" }}\n'
                  f'\n  [node.components.Material]\n  albedo = [0.42, 0.34, 0.24]\n'
                  f'  roughness = 0.85\n')
    for n, ps in (("rest", None), ("hold", S.HOLD)):
        write(os.path.join(SCENES, f"fp_{n}.loom"),
              env(820 if n == "rest" else 821)
              + body("Deckhand", "Root", ps) + GROUND + posts
              + cam((0.0, S.EYE_Y, 0.0), (0.0, 0.0, 0.0), 75.0))

    # THE MIRROR, at the demo's own shed coordinates.  Not a new pass and not
    # a new ADR: `tracedEnvironment` (scene.slang:2731) runs for every opaque
    # fragment, and at metallic 1.0 the f0 IS the albedo, so a light grey metal
    # at roughness 0 hands back ~95% of the traced radiance.  The blueprint
    # measured that with primitive stand-ins; this asks the only new question,
    # which is whether 27 imported OBJ leaves land in the TLAS the same way.
    mirror = ""
    for nm, sc, z, a, met, rough in (
            ("MirrorFrame", (0.43, 0.88, 0.025), 2.575,
             (0.055, 0.050, 0.045), 0.0, 0.62),
            ("Mirror", (0.40, 0.85, 0.010), 2.552,
             (0.95, 0.96, 0.97), 1.0, 0.0)):
        # The blueprint's y 2.40 is RIG-LOCAL -- the demo's deck top is 1.40 --
        # so on this test scene's ground plane the glass has to come down by
        # exactly that, to a centre at eye height.  Transplanted unchanged it
        # sits 0.75 m above the eye and reflects nothing but sky and the crown
        # of his own hat (r1/out/mirror_first.png, taken before this).
        mirror += (f'\n[[node]]\nname = "{nm}"\nparent = "Root"\n'
                   f'transform = {{ pos = [0.0, 1.65, {z}], '
                   f'scale = [{sc[0]}, {sc[1]}, {sc[2]}] }}\n'
                   f'\n  [node.components.MeshRenderer]\n  mesh = {{ asset = "box" }}\n'
                   f'\n  [node.components.Material]\n'
                   f'  albedo = [{a[0]}, {a[1]}, {a[2]}]\n  metallic = {met}\n'
                   f'  roughness = {rough}\n  porosity = 0.0\n')
    mirror += ('\n[[node]]\nname = "Wall"\nparent = "Root"\n'
               'transform = { pos = [0.0, 1.30, 3.20], '
               'scale = [3.0, 1.30, 0.60] }\n'
               '\n  [node.components.MeshRenderer]\n  mesh = { asset = "box" }\n'
               '\n  [node.components.Material]\n  albedo = [0.16, 0.15, 0.14]\n'
               '  roughness = 0.80\n')
    # He stands 1.7 m off the glass, as he would on the demo's deck, and the
    # camera is his own eye.
    stand = '\n[[node]]\nname = "Man"\nparent = "Root"\nprefab = "deckhand"\n' \
            'transform = { pos = [0.0, 0.0, 0.85], rot_euler = [0.0, 180.0, 0.0] }\n'
    write(os.path.join(SCENES, "mirror_first.loom"),
          env(830, with_assets=False) + ref + stand + GROUND + mirror
          + cam((0.0, S.EYE_Y, 0.85), (0.0, 180.0, 0.0), 60.0))
    write(os.path.join(SCENES, "mirror_third.loom"),
          env(831, with_assets=False) + ref + stand + GROUND + mirror
          + cam((1.70, 1.60, -0.40), (-8.2, 142.3, 0.0), 55.0))

    # CO-OP LEGIBILITY, on the REAL prefab and the demo's night lighting.
    # The blueprint measured this on a primitive stand-in, and the hat -- the
    # one feature that has to survive to 28 m -- is the part round 2 changed
    # most.  At 75 deg fov and 1080 lines a 1.80 m figure is 633 px at 2 m,
    # 106 px at 12 m and 45 px at 28 m, and the demo rig is 24 x 14 m, so
    # corner to corner a teammate is 45 px tall with a 9-pixel head.
    night = ('[scene]\nformat = 1\n'
             'id = "d3c8a5e0-9000-4000-8000-000000000860"\n'
             '\n[[node]]\nname = "Root"\n'
             '\n  [node.components.Environment]\n'
             '  sun_direction = [0.28, 0.42, -0.86]\n'
             '  sun_strength = 0.55\n  sun_color = [0.72, 0.78, 0.92]\n'
             '  ambient = 0.20\n  sky_zenith = [0.045, 0.060, 0.085]\n'
             '  sky_horizon = [0.115, 0.130, 0.150]\n'
             '  fog_density = 0.010\n  fog_falloff = 0.06\n'
             '\n[[node]]\nname = "Deck"\nparent = "Root"\n'
             'transform = { pos = [0.0, -0.10, -16.0], '
             'scale = [7.0, 0.10, 22.0] }\n'
             '\n  [node.components.MeshRenderer]\n  mesh = { asset = "box" }\n'
             '\n  [node.components.Material]\n  albedo = [0.19, 0.17, 0.15]\n'
             '  roughness = 0.86\n  porosity = 0.72\n')
    for i, (x, z, yaw) in enumerate(((-0.55, -4.0, 168.0),
                                     (0.75, -12.0, 152.0),
                                     (-1.30, -28.0, 190.0))):
        night += (f'\n[[node]]\nname = "Mate{i}"\nparent = "Root"\n'
                  f'prefab = "deckhand"\n'
                  f'transform = {{ pos = [{x}, 0.0, {z}], '
                  f'rot_euler = [0.0, {yaw}, 0.0] }}\n')
    write(os.path.join(SCENES, "coop.loom"),
          night + ref + cam((0.0, 1.65, 0.0), (-2.0, 0.0, 0.0), 75.0))

    print("scenes  %s  (%d files)" % (SCENES, len(os.listdir(SCENES))))
    print("NECK_Y %.3f  head unit %.3f  heads %.2f  leg %.3f (%.3f H)  eye %.3f"
          % (S.NECK_Y, S.H - S.NECK_Y, S.H / (S.H - S.NECK_Y), S.CROTCH,
             S.CROTCH / S.H, S.EYE_Y))
    worst = S.SH_X + S.ARM_R * 1.14
    print("widest rest point %.4f m from the spine vs CharacterController "
          "radius 0.350 -> %s" % (worst, "inside" if worst < 0.35 else "OUTSIDE"))


if __name__ == "__main__":
    main(sys.argv[1] if len(sys.argv) > 1 else
         os.path.abspath(os.path.join(DIR, "..", "..")))
