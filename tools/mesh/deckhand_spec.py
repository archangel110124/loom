#!/usr/bin/env python3
"""THE DECKHAND — the dimension table, in metres, Loom axes.

Loom is Y up.  Authored forward is -Z (play.rs:3710, "Authored facing is -Z,
so forward is -Z").  So boot toes point -Z, the sou'wester brim sweeps long
toward +Z, the creel rides on +Z.

Everything the Blender build and the .loom emitter both need lives here and
NOWHERE ELSE.  No bpy import, so both can read it.

Every leaf mesh is authored at FINAL SIZE in its own node's local frame, so a
leaf node's scale is [1,1,1] -- except the two reused parts (limb, ball) that
genuinely need one number per instance.  That keeps rule "a pivot never
carries a scale" trivially true and it keeps compose()'s column-scaling from
shearing anything.
"""
import math

# ---- skeleton ------------------------------------------------------------
H          = 1.800          # crown of the hat
CROTCH     = 0.800          # Hips pivot,  0.444 H
THIGH      = 0.412
SHIN       = 0.308
ANKLE_Y    = 0.080
PELVIS     = 0.246          # crotch -> waist
CHEST      = 0.394          # waist  -> neck plane
NECK_Y     = CROTCH + PELVIS + CHEST        # 1.440
EYE_Y      = 1.650

SH_HW      = 0.223          # shoulder half-width, at the shoulder cap only
# THE ARM SLOT IS CUT OUT OF THE TORSO, not bought with shoulder width.  A
# hanging arm's inner edge is SH_X - ARM_R = 0.211; the torso has to be
# NARROWER than that below the armpit or the arms sit inside its outline and
# the front view is four parallel vertical bars.  Wide at the shoulder cap,
# in at the ribs, is also what an oilskin over shoulders does.
# SH_X was 0.276.  Two reasons it came in, and the first is a hard limit: the
# shoulder ball had to grow (see BALL_INRADIUS) and 0.276 + the new ball is
# 0.351, which is OUTSIDE the CharacterController's 0.350 radius.  The second
# is the front silhouette -- at 0.276 the arms hung far enough outboard to
# leave a slot the full length of the torso, and arms-out/legs-together is the
# opposite of a natural stand.
#
# ROUND 3 PUTS IT BACK OUT, to 0.272, and the reason the first cut was wrong is
# that it closed the ARM SLOT -- the gap of background between the inner edge
# of a hanging arm and the side of the torso.  Front-on the figure became four
# parallel vertical bars: arm inner edge 0.201 against a chest half-width of
# 0.223, so the arms were INSIDE the torso outline from armpit to hem and the
# silhouette had no shoulder in it.  0.272 + the shoulder ball is 0.339, still
# inside the CharacterController's 0.350, and the rest of the slot comes from
# taking the torso IN below the armpit (build_deckhand.build_torso) rather than
# pushing the arms further out, which is the half that has no radial budget.
SH_X       = 0.272
SH_DROP    = 0.056          # shoulder pivot below the neck plane
HIP_X      = 0.118
# HIP_HW was 0.183 -- NARROWER THAN THE PAIR OF THIGHS.  A thigh sits at
# x 0.118 with radius 0.105, so its outer edge is 0.223 and the hip ball's is
# 0.233: 50 mm of leg and joint hanging outboard of the bib it comes out of,
# which front-on reads as saddlebags rather than as hips.  The pelvis is the
# widest part of the lower body on a real figure and it is now wider than the
# thighs it feeds.
HIP_HW     = 0.220
# ...and the other half of the same fix is that a 0.105 m thigh RADIUS is a
# 210 mm thigh on a 1.8 m man.  Slimming it to 0.094 pulls the leg inboard of
# the widened bib, opens 48 mm of daylight between the two legs where there
# were 26, and shrinks all four leg joint balls with it -- one number, three
# of the critique's smaller notes.
# The side silhouette was a vertical pole of near-constant width: chest 0.130,
# pelvis 0.117 and thigh 0.105 are all within 25 mm, so there was no chest, no
# belly and no buttock, and every millimetre of profile came from the hat, the
# creel and the lamp.  The chest is deeper now and the extra mass is FORWARD
# (CHEST_CZ), which is where a chest is; the pelvis bulges rearward.
#
# ROUND 3 GOES FURTHER, because 0.163 against 0.128 was still a slab: 35 mm of
# difference across the whole trunk on a 1.8 m figure.  Mask the hat and the
# creel out of r2/out/sil_side.png and what is left does not read as a person
# -- from shoulder to boot both the front and the rear edge are straight
# vertical lines, and the only two shapes in the entire profile are furniture.
# A profile is where a body's mass reads; a front view can hide a slab behind
# its own width and a side view cannot.
CHEST_HD   = 0.200
CHEST_CZ   = -0.034         # torso rings sit forward of the spine
PELV_HD    = 0.158
PELV_CZ    = +0.022         # pelvis rings sit rearward of the spine
UPPER      = 0.310          # shoulder -> elbow
FORE       = 0.290          # elbow -> wrist
HAND_L     = 0.162          # the mitten's own length, cuff to tip
ARM_R      = 0.061
FORE_R     = 0.054
# HAND_R was 0.055 against a FORE_R of 0.054 -- the mitten was ONE MILLIMETRE
# wider than the sleeve it came out of, so seen end-on down your own arm in
# first person it was not a hand, it was the end of a stick.  The hands are
# the second-brightest value in the palette by design and the thing a
# first-person fisherman looks at all session; they get to be hands.
HAND_R     = 0.080
LEG_R      = 0.094
COLLAR_R   = 0.118
NECK_R     = 0.072
CREEL      = (0.140, 0.115, 0.084)
RIB_HW     = 0.186          # torso half-width from the armpit to the hem
CREEL_Z    = +0.236         # pulled in with the deeper chest; see the radial
                            # check at the bottom of this file

# ---- the sou'wester ------------------------------------------------------
# Round 1 built a Brodie helmet: a shallow 0.135 crown on a 0.232 flat disc,
# a brim/dome ratio of 1.40, which is the WW1 Tommy ratio.  Front and rear
# brim edges came out at exactly the same height, so the rear sweep never read
# in profile and the front silhouette -- the test that matters -- said soldier.
#
# A sou'wester is TALLER IN THE CROWN THAN THE BRIM IS WIDE, short at the
# front, turned down hard at the SIDES (it sheds water past your ears) and
# swept into a long flat tail at the back.  All four of those are numbers here.
CROWN_H    = 0.206          # apex above the brim plane; > BRIM_R by design
CROWN_R    = 0.172          # widest point of the crown
BRIM_R     = 0.178          # FRONT brim radius, measured from the head axis
BRIM_SIDE  = 1.28           # x radius at the ears,  = 0.228
BRIM_BACK  = 1.62           # z radius at the tail,  = 0.308
# 1.72 was the first value and it put the tail's outer edge at z 0.341,
# plus HatM's 10 mm offset = 0.351 -- ONE MILLIMETRE OUTSIDE the
# CharacterController's 0.350 radius.  The radial check below did not catch
# it because the hat was not in the radial check; it is now.
HAT_OPEN   = 0.110          # head opening below the brim plane
# Droop, in degrees, applied to the brim's overhang past CROWN_R.  The sides
# curl over the ears; the tail stays nearly flat so it still reads as a tail.
DROOP_BASE = 8.0
DROOP_SIDE = 50.0
DROOP_BACK = -6.0
# DROOP_SIDE was 26 and it did not read.  The droop is applied to the brim's
# OVERHANG PAST CROWN_R, and at BRIM_SIDE 1.15 that overhang was 47 mm, so 26
# deg bought 23 mm of turn-down on a 219 mm brim -- invisible.  Front-on the
# hat was still a dome sitting on a flat horizontal disc, which is the Brodie
# read, and the front silhouette still said soldier.  A rain hat's sides come
# down PAST THE EARS: wider side brim to have something to bend, then bend it
# properly.  The tail is what stays flat.
# Placement: the crown apex lands exactly on H, so the hat's local origin (the
# brim plane) sits CROWN_H below it.
BRIM_Y     = H - CROWN_H                    # 1.594
HAT_Y      = BRIM_Y - NECK_Y                # 0.154, HatM local y under Neck
HAT_Z      = 0.000          # HatM local z; kept at 0 so the tail's radial
                            # reach is the brim number and nothing else
# THERE IS A NECK AGAIN, AND ROUND 2 OVERCORRECTED INTO HAVING NONE.
#
# Round 1 left 101 mm of bare cylinder and the figure read as a hat on a stick;
# round 2 closed it to a 14 mm OVERLAP, so the hat's skirt rested on the storm
# collar and from every angle the head sat directly on the shoulders.  All six
# reference games keep a visible neck gap, and the reason is silhouette
# mechanics rather than anatomy: the gap is the only thing separating
# head-mass from body-mass in an outline, and without it the hat is not a head,
# it is the top of the torso.
#
# NECK_GAP is the number, it is asserted at the bottom of this file, and both
# the hat's opening and the collar's height are derived from it so it cannot be
# broken by moving either one.
NECK_GAP   = 0.034          # bare neck between the collar's top and the brim
HEAD_OPEN_Y = BRIM_Y - HAT_OPEN             # 1.484
COLLAR_TOP = HEAD_OPEN_Y - NECK_GAP         # 1.450
COLLAR_H   = 0.020                          # half-height of the brass ring
COLLAR_WY  = COLLAR_TOP - COLLAR_H          # WORLD y of the ring's centre
NECK_TOP   = HEAD_OPEN_Y + 0.020            # the gusset runs INTO the hat

# ---- the head, and it is a plug rather than a portrait ------------------
#
# THE HAT WAS AN EMPTY BELL AND FOUR SEPARATE REVIEWS SAW STRAIGHT UP INTO IT.
# `deckhand_hat.obj` closes its crown with a fan at the brim plane over a disc
# of radius HAT_OPEN_R -- so from any eye at or below brim height what you get
# is that disc, lit, in sou'wester ochre.  Not a shadow: a surface, at close to
# the outside's value, which reads as a prop balanced on a stick and is the
# first thing the eye goes to in the mirror.
#
# The fix is a dark cylinder plugging the opening, not a head sticking out
# below it.  NECK_GAP is a deliberate 34 mm of bare neck and it is the only
# thing separating head-mass from body-mass in the outline; a ball big enough
# to read as a face would close it and undo round 3.  So the plug sits ENTIRELY
# inside the hat, from the opening plane upward, and what it changes is that
# the cavity is dark instead of ochre.
#
# It carries no face by design.  A co-op horror deckhand whose face you cannot
# see is the tone; a modelled face would be a different character.
HEAD_R     = 0.100          # = the crown's closing disc, so nothing peeks past
HEAD_H     = 0.075          # half-height; top lands 1.634, well under CROWN_R
HEAD_Y     = HEAD_OPEN_Y + HEAD_H - NECK_Y  # local y under Neck

# ---- joint balls ---------------------------------------------------------
# Round 1 sized every ball against its neighbour's NOMINAL radius and four of
# the eight came out smaller than the limb they were supposed to cap, so the
# limb's flat 10-gon rim sawed through the sphere and the knees and elbows
# rendered as a hard zig-zag fracture ring.  Two measured numbers fix it:
#
#   BALL_INRADIUS  the icosphere's guaranteed radius is NOT 1.0 and not the
#                  0.951 bounding box either -- it is the minimum distance
#                  from the centre to a FACE PLANE, measured off
#                  deckhand_ball.obj at 0.934171.  A subdivision-2 icosphere
#                  is 80 flat triangles and it is the flats that show.
#   LIMB_BULGE     deckhand_limb.obj's widest ring is 1.06, not 1.00, so a
#                  limb of "radius r" is really r * 1.06 at its shoulder end.
#
# Fixing it costs ZERO triangles.  Subdividing the icosphere instead would be
# 1,920 triangles for the same silhouette.
BALL_INRADIUS = 0.934171
LIMB_BULGE    = 1.00        # deckhand_limb.obj's widest ring, now flat-sided
BALL_MARGIN   = 1.02        # how far the ball stands proud of the limb
# 1.06 x 1.08 put the hip and knee balls at 1.29x the leg radius and the legs
# read as a chain of boulders -- the opposite failure to round 1's sawtooth and
# just as wrong.  Removing the limb's own bulge and cutting the margin gives
# 1.09x: a bulge you read as a joint, with the enclosure still PROVEN rather
# than eyeballed.  The guaranteed margin is what may not go to zero, not the
# proudness.


def ball_scale(*neighbour_radii):
    """The uniform scale for a joint ball that must cap these limb radii.

    Guaranteed enclosing test, which is what checks.sh asserts:
        BALL_INRADIUS * ball_scale > LIMB_BULGE * max(neighbour_radii)
    """
    return max(neighbour_radii) * LIMB_BULGE / BALL_INRADIUS * BALL_MARGIN


SHIN_R = LEG_R * 0.88

BALL_SHOULDER = ball_scale(ARM_R)
BALL_ELBOW    = ball_scale(ARM_R, FORE_R)
BALL_HIP      = ball_scale(LEG_R)
BALL_KNEE     = ball_scale(LEG_R, SHIN_R)

# ---- the boot ------------------------------------------------------------
# Round 1's boot was a flat wedge 40-72 mm tall that stopped at the ankle: a
# shoe, not a boot, and from the side it read as a flipper.  A deck boot has a
# SHAFT, and the shaft is also the rearward mass at the bottom of the side
# silhouette that the flat leg cannot provide.
BOOT_Y     = -0.040         # BootLM local y under the Ankle pivot
BOOT_Z     = -0.045         # ...and local z; the foot leads the ankle
SOLE_Y     = -0.040         # lowest point of deckhand_boot.obj, = ground at rest
BOOT_TOP   = 0.155          # shaft top, mesh local -> world 0.195
BOOT_TOP_R = 0.120          # must swallow the shin through the ankle's swing:
                            # SHIN_R + BOOT_TOP * sin(ANK_MAX) = 0.0924 + 0.024

# ---- materials: linear RGB, metallic, roughness, porosity ----------------
OIL   = ((0.090, 0.105, 0.120), 0.0, 0.42, 0.10)   # black oilskin
WADER = ((0.115, 0.125, 0.115), 0.0, 0.55, 0.05)   # green rubber waders
SOU   = ((0.520, 0.395, 0.095), 0.0, 0.30, 0.04)   # the ochre sou'wester
BOOT  = ((0.045, 0.048, 0.050), 0.0, 0.80, 0.20)
HAND  = ((0.560, 0.470, 0.390), 0.0, 0.62, 0.35)
BRASS = ((0.560, 0.455, 0.205), 0.9, 0.34, 0.00)
SKIN  = ((0.030, 0.029, 0.031), 0.0, 0.70, 0.05)   # the head: a void, on
                                                   # purpose -- see HEAD_R
LENS  = ((0.880, 0.860, 0.780), 0.0, 0.18, 0.00)
# WICKER: porosity was 0.70 and it is the wetness-darkening term, so the
# creel -- the second-largest feature in the silhouette -- swung from light
# tan in the mirror to near-black on a wet deck.  A varnished wicker basket
# does not drink; 0.28 keeps one value across both.
WICKER= ((0.215, 0.170, 0.100), 0.0, 0.88, 0.28)

# ---- part table ----------------------------------------------------------
MESHES = ("hat", "torso", "pelvis", "creel", "boot", "hand", "limb", "ball")

# A pivot's local x, in millimetres, purely so a shared script can read its own
# side off sign(position[0]).  Round 1 authored the elbow, knee, ankle and
# hand pivots at x = 0.0 on BOTH sides, so `if position[0] < 0.0` was FALSE on
# both and six of the fourteen joints could not be addressed at all.  The knee
# and elbow survived by luck -- they are rectified and |sin(p)| == |sin(p+pi)|
# -- but the ankle is not rectified, so both feet rolled in unison while the
# legs were half a cycle apart.  One millimetre is invisible and makes the
# contract uniform instead of accidentally-correct at four joints of six.
SIDE_EPS = 0.001


def limb_scale(length, radius):
    """A limb leaf sits at the bone's midpoint; the mesh is unit half-length."""
    return (radius, length * 0.5 * OVERRUN, radius)


OVERRUN    = 1.12           # a limb segment is modelled 12% longer than its bone


# ---- the walk ------------------------------------------------------------
# THE KNEE IS HALF-WAVE RECTIFIED, NOT FULL-WAVE, AND IT HAS TWO TERMS.
#
# Round 1 used `abs(34 * sin(phi - 0.9))`.  A full-wave rectified sine has
# period pi -- TWO knee bends per stride, one of them in the middle of the
# stance phase -- so the leg was 25 deg bent at exactly the moment it should
# have been carrying weight straight.  Measured consequence, via r2/legfk.py:
# the supporting sole rode between -47.6 mm and +69.5 mm, NEVER touched the
# deck at any phase, and swung over a 117 mm range.  On a wooden deck with
# footstep audio that reads as skating immediately.  `abs()` was described as
# the trick that separates a walk from a puppet flapping; the real trick is
# that the joint bends ONE WAY, and max(0, sin) gives that AND the right
# frequency.
#
# The second term is forced by geometry, not taste.  The hip swings +-30 deg,
# so the hip-to-ground distance the leg must span is sqrt(h^2 + d^2): longest
# at heel strike and toe-off, shortest under the body.  The knee therefore has
# to be STRAIGHTEST AT THE EXTREMES AND BENT IN MID-STANCE, which is the real
# gait's stance flexion wave and which one half-wave cannot produce at any
# amplitude.  Fitting with a single term pinned three parameters against their
# bounds and stalled at 24 mm.  Both terms are max(0, .) so the sum is never
# negative and the knee can never hyperextend, however they are phased.
#
# Every constant below was fitted by r2/gait.py against r2/legfk.py, which
# reproduces `loom sim`'s foot heights to 4.3e-5 m.  Result: vertical plant
# error 11.8 mm, horizontal skate 16.9 mm, both inside the thickness and
# length of the sole itself.
# THE ELBOW BENT BACKWARDS IN ROUND 2, AT EVERY JOINT, IN BOTH THE WALK AND
# THE HOLD, and it is the single structural defect of that round.
#
# compose() maps (0,-L,0) under a rot-X of t to (0, -L cos t, -L sin t), so
# z = -L sin t and +Z IS BEHIND: a POSITIVE angle swings a segment FORWARD.
# The knee is correctly negative because a knee flexes backward.  The elbow is
# its mirror -- an elbow flexes FORWARD -- so an elbow angle must be POSITIVE,
# and round 2 wrote `-(abs(26 sin) + 8)` at both elbows and -44/-64 in the
# hold.  That is hyperextension, and measured with r3/armfk.py it put the
# forward hand 0.110 m ahead of the shoulder against 0.448 m behind it: a 4.1x
# asymmetry, with the rear hand riding HIGHER than the forward one.  Two hands
# at different heights at the same instant is the signature.
#
# SECOND BUG IN THE SAME EXPRESSION: abs(26 sin(w+pi)) == abs(26 sin(w)), so
# BOTH elbows bent in unison, twice per stride.  That is exactly the error
# removed from the knee one round earlier, in a line whose own comment names
# the identity without noticing what it costs.  Half-wave rectification fixes
# the sign, the frequency and the left/right split in one expression, and it
# is the same tool for the same reason: a hinge bends one way only.
EL_FLOOR   = 11.0           # an arm at rest is never quite straight
EL_K       = 21.0           # flexion, on the forward half of its own swing
EL_PH      = -0.45          # ...peaking a little after the shoulder does
SH_A       = 23.0           # shoulder swing amplitude
# YAW AND ROLL WERE BELOW THE NOISE FLOOR OF THE PICTURE.  Measured off
# r2_walk.png with r3/headmove.py: over eight frames the hat's centroid moved
# 2.1 px horizontally and its bounding width changed by 2.6% -- so the whole
# upper body, hat, collar, creel and torso, held ONE rigid attitude while the
# legs walked underneath it.  A body walks by catching itself; if the head
# does not turn, lag or drop into the stance side, the legs read as a machine
# carrying a mannequin, which is the loudest programmer-walk tell there is.
# 4 deg of counter-yaw changes a projected width by 2.6%; 9 does not.
YAW_A      = 9.0            # shoulder counter-yaw against the pelvis
ROLL_A     = 5.0            # pelvis roll into the stance side
NECK_A     = 6.0            # ...and the head lags the shoulders, which is the
NECK_LAG   = 0.55           # one line that does most of the work
KNEE_K     = 60.4           # swing flexion
KNEE_K2    = 12.0           # stance flexion wave
KNEE_FLOOR = 3.7            # a leg is never quite straight
ANK_A      = 14.0           # heel strike to toe-off roll
ANK_PH     = 0.00
BOB_0      = -0.0001         # fitted by r2/bob.py against the vertical float
BOB_A      = 0.0338         # alone; the horizontal skate is the hip's problem
BOB_PH     = 1.07           # and is not the bob's to fix -- see that file
LEAN       = 3.2            # forward pitch on the spine; round 1 was a march
# ...AND A CONSTANT LEAN IS A POSE, NOT A MOTION.  The round-2 critique read
# the walk sheet as "an articulated lower half carrying an inert upper half"
# and it was right, but the diagnosis it offered -- more counter-yaw and more
# pelvis roll -- cannot be seen in the view the sheet is rendered from.  The
# sheet is a SIDE view, and from +X a Spine rot-Y and a Hips rot-Z both move
# the hat straight into the camera.  Measured with r3/headmove.py, which
# reproduces the critique's own hand numbers on the round-2 frames (2.5% and
# 1.6 px against its 2.6% and 2.1 px): raising the yaw from 4 to 9 degrees and
# the roll from 2.4 to 5 moved the hat 2.4 px.  They are still worth having --
# the FRONT sheet is where the roll swings the hat 148 mm -- but the channel
# that carries the side view is the LEAN, and it was nailed to 3.2.
#
# A trunk pitches forward twice per stride, at each heel strike, for the same
# reason the pelvis drops twice: that is when the body is catching itself.
# 3 degrees moves the hat 63 mm across the frame.
LEAN_A     = 3.0
LEAN_PH    = 0.60
ANK_MAX    = 12.0           # the swing the boot shaft must still swallow


def knee_curve(w):
    """Half-wave rectified twice, AT PINNED PHASES.

    max(0, cos w) is exactly the swing window and max(0, -cos w) exactly the
    stance window, so the knee is straight at heel strike, dips through the
    stance flexion wave, straightens at toe-off and folds through swing.  The
    phases are pinned rather than fitted because a fitted pair scored 17 mm on
    the foot-plant metric by putting 48 deg of flexion at HEEL STRIKE -- a
    perfectly planted foot and an obvious crouch on the contact sheet.  A
    plant metric cannot tell a walk from a squat."""
    return -(KNEE_K * max(0.0, math.cos(w))
             + KNEE_K2 * max(0.0, -math.cos(w))
             + KNEE_FLOOR)


def elbow_curve(w):
    """POSITIVE and half-wave rectified, exactly like the knee and for exactly
    the same reason: a hinge bends one way, once per stride, on its own side."""
    return EL_FLOOR + EL_K * max(0.0, math.sin(w + EL_PH))


def pose(t):
    """t in 0..1 = one stride.  Returns the joint angles in degrees.

    Contralateral, and none of the three phase relationships is negotiable:
    legs pi apart, each arm pi from the leg on its OWN side, knee and ankle
    lagging the hip."""
    w = 2.0 * math.pi * t
    return dict(
        hipL=30.0 * math.sin(w),
        hipR=30.0 * math.sin(w + math.pi),
        kneeL=knee_curve(w),
        kneeR=knee_curve(w + math.pi),
        ankL=ANK_A * math.sin(w + ANK_PH),
        ankR=ANK_A * math.sin(w + math.pi + ANK_PH),
        shL=SH_A * math.sin(w + math.pi),
        shR=SH_A * math.sin(w),
        elL=elbow_curve(w + math.pi),
        elR=elbow_curve(w),
        spL=0.0, spR=0.0,
        yaw=-YAW_A * math.sin(w + math.pi),
        nyaw=NECK_A * math.sin(w + math.pi - NECK_LAG),
        roll=ROLL_A * math.sin(w),
        lean=LEAN + LEAN_A * math.sin(2.0 * w + LEAN_PH),
        bob=BOB_0 + BOB_A * math.sin(2.0 * w + BOB_PH),
    )


REST = dict(hipL=0.0, hipR=0.0, kneeL=0.0, kneeR=0.0, ankL=0.0, ankR=0.0,
            shL=0.0, shR=0.0, elL=0.0, elR=0.0, spL=0.0, spR=0.0,
            yaw=0.0, nyaw=0.0, roll=0.0, bob=0.0, lean=0.0)

# Holding the rod at ready.  Blueprint 5, `state.hold >= 1` replaces the walk
# term on the four arm joints.
#
# THE BLUEPRINT'S SIGNS ARE WRONG AND THIS IS THE FIX.  It specifies
# shR = -52, elR = -78, shL = -40, elL = -95.  compose() maps (0,-L,0) under a
# rot-X of t to (0, -L cos t, -L sin t), so z = -L sin t is POSITIVE for a
# negative t -- and +Z is BEHIND.  Measured with r2/handcheck.py, that pose
# puts the right hand at z +0.539 and the left at +0.491: both hands about
# half a metre behind the eye, with the arms pointing backwards.
#
# The walk uses the same channel the other way round -- `shR = 28 sin(w)` is
# positive at w = pi/2 and swings the arm FORWARD, which is correct -- so the
# two halves of the blueprint disagree about which way positive is, and the
# hold is the half that is wrong.
#
# This is the whole explanation for a first-person frame with no hands in it.
# Round 1 reported `fp_hold.png` as "two distant fence posts with a pale
# sliver" and read it as the mitten being too thin; the mitten was too thin,
# and it was also behind him.  Two defects, one image, and only one of them
# was in the geometry.
#
# ...AND ROUND 2'S REPLACEMENT WAS WRONG THE OTHER WAY, which is the more
# interesting failure of the two.  It fixed the SHOULDER's sign and left the
# ELBOW's alone, then raised the shoulder to 112 deg to compensate for an
# elbow that was folding backwards -- so both upper arms came up ABOVE
# horizontal with the elbows at ear height, the mittens beside the head and
# the rod hanging tip-down behind him.  r2/r2_rod.png is a man surrendering.
#
# And it was signed off by an instrument.  handcheck.py scored that pose a
# pass -- "both hands in frame, 26 deg below the eye, grip separation 0.528 m"
# -- because a hand-in-frustum metric cannot tell a fishing grip from a
# surrender, in precisely the way a foot-plant metric cannot tell a walk from
# a squat.  That lesson was written down one round earlier, in this file, and
# then walked into again.  The eye-offset sweep that followed is downstream of
# the same error: an eye offset can only CROP the arms, it cannot move them,
# so "no hand radius fixes this, the fix is scene-side" was answering a
# question about geometry that was really a question about pose.
#
# THE SPLAY IS NEW AND IT IS WHAT MAKES IT A GRIP.  Two arms swinging in the
# sagittal plane alone put the hands 0.52 m apart, which is shoulder width and
# not a rod.  `spL/spR` is the shoulder's rot-Z; under Y-X-Z it applies BEFORE
# the swing, so it tucks the elbow inboard and carries the hand toward the
# centre line without tilting the swing plane.  It is authored at zero and the
# walk leaves it there.
#
# Solved with r3/armfk.py, which is the FK the round-2 instrument was missing:
# hands at x +-0.114, y 1.238, z -0.483 -- 0.23 m apart on the centre line,
# between the hips and the sternum, 40 deg below the eye, with both elbows at
# y 1.13, a quarter of a metre BELOW the shoulders.
HOLD = dict(REST, shR=32.0, elR=78.0, spR=-22.0, wrR=-10.0,
            shL=32.0, elL=78.0, spL=+22.0, wrL=-10.0)


# ---- the one check that is arithmetic, not a render ----------------------
def radial_check():
    """Nothing may poke out of the CharacterController's 0.350 m radius.

    Every entry is (name, distance from the spine axis).  A superellipse
    |x/a|^e + |z/b|^e = 1 reaches furthest from the axis at its CORNER, where
    x/a = z/b = 0.5**(1/e) -- checking only the face half-widths misses it.
    """
    out = []
    out.append(("shoulder pivot + ball", SH_X + BALL_SHOULDER))
    out.append(("hip pivot + ball", HIP_X + BALL_HIP))
    out.append(("boot shaft outer", HIP_X + BOOT_TOP_R))
    s = 0.5 ** (1.0 / 3.8)
    out.append(("torso corner", math.hypot(s * SH_HW, abs(CHEST_CZ) + s * CHEST_HD)))
    s = 0.5 ** (1.0 / 3.2)
    cw, _ch, cd = CREEL
    out.append(("creel lid corner",
                math.hypot(s * cw * 1.06, CREEL_Z + s * cd * 1.06)))
    # The hat's rear tail is a genuine contender and was missed once.  The
    # brim's outer edge row is BRIM_R + 0.008 before the sweep multiplies it.
    out.append(("hat rear tail", (BRIM_R + 0.008) * BRIM_BACK + HAT_Z))
    out.append(("hat side brim", (BRIM_R + 0.008) * (1.0 + (BRIM_SIDE - 1.0)) * 1.03))
    return out


if __name__ == "__main__":
    print("balls   shoulder %.4f  elbow %.4f  hip %.4f  knee %.4f"
          % (BALL_SHOULDER, BALL_ELBOW, BALL_HIP, BALL_KNEE))
    for nm, r, in ((("shoulder"), (ARM_R,)), ("elbow", (ARM_R, FORE_R)),
                   ("hip", (LEG_R,)), ("knee", (LEG_R, SHIN_R))):
        b = ball_scale(*r)
        need = LIMB_BULGE * max(r)
        print("  %-9s guaranteed %.4f  vs limb %.4f  margin %+.1f mm"
              % (nm, BALL_INRADIUS * b, need, (BALL_INRADIUS * b - need) * 1000))
        assert BALL_INRADIUS * b > need, nm
    print("hat     crown %.3f  brim front %.3f side %.3f back %.3f"
          % (CROWN_H, BRIM_R, BRIM_R * BRIM_SIDE, BRIM_R * BRIM_BACK))
    assert BRIM_R < CROWN_H, "brim is wider than the crown is tall -> helmet"
    assert BRIM_R < BRIM_R * BRIM_BACK * 0.7, "rear sweep will not read"
    print("        head opening %.3f  collar top %.3f  -> %.0f mm of neck"
          % (HEAD_OPEN_Y, COLLAR_TOP, NECK_GAP * 1000))
    assert 0.025 <= NECK_GAP <= 0.040, \
        "the head fuses into the shoulders (0) or sits on a stick (101 mm)"
    print("slot    arm inner edge %.3f vs rib half-width %.3f -> %+.0f mm"
          % (SH_X - ARM_R, RIB_HW, (SH_X - ARM_R - RIB_HW) * 1000))
    assert SH_X - ARM_R > RIB_HW + 0.015, "no arm slot: four parallel bars"
    print("hips    pelvis %.3f vs thigh outer %.3f -> %+.0f mm"
          % (HIP_HW, HIP_X + LEG_R, (HIP_HW - HIP_X - LEG_R) * 1000))
    assert HIP_HW > HIP_X + LEG_R, "the thighs hang outboard of the bib"
    print("hand    mitten r %.3f vs sleeve r %.3f  -> %.2fx" % (HAND_R, FORE_R, HAND_R / FORE_R))
    assert HAND_R > FORE_R * 1.35, "the mitten reads as the end of a stick"
    print("radial  (CharacterController radius 0.350)")
    for nm, d in radial_check():
        print("  %-22s %.4f  %s" % (nm, d, "inside" if d < 0.350 else "*** OUTSIDE"))
        assert d < 0.350, nm
    print("OK")
