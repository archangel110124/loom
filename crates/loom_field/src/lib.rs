//! Analytic fields, authored once and emitted twice.
//!
//! **The failure this exists to prevent is the worst one in the backlog.**
//! Water, rain and wind each need the same field sampled on the CPU (physics,
//! gameplay, the determinism hash) and on the GPU (every vertex and pixel that
//! moves with it). Write those twice and you get two implementations that are
//! each internally correct and silently disagree — a boat that rides waves the
//! player cannot see, grass leaning one way while the rain streaks lean
//! another. Nothing errors. Nothing is obviously wrong. It just never quite
//! lines up, and there is no line of code to blame.
//!
//! So a field is not written in Rust *or* in Slang. It is written once, here,
//! as an expression tree, and both are generated from it. They cannot disagree
//! about what the field *is*; they can only disagree about floating point,
//! which is what the agreement test measures.
//!
//! **Deliberately tiny.** This is not a shading language. It is the smallest
//! set of operations that expresses a wind field, an ocean spectrum and a rain
//! drift vector: arithmetic, `sin`/`cos`, `min`/`max`, `abs`, and the position
//! and time inputs. Everything harder belongs in a shader written by hand,
//! which is fine as long as it is not a field two sides have to agree on.
//!
//! It is also a `ponytail:` — an AST interpreted on the CPU is slower than
//! straight-line Rust. A wind field is sampled a few times per tick, not per
//! pixel, so it does not matter yet. If it ever does, the seam is [`Field`]:
//! the same tree can emit Rust source instead of being walked.

pub mod noise;
pub mod wind;

/// One scalar expression over position and time.
///
/// Cloned rather than referenced everywhere: a field is built once at startup
/// and these trees are tens of nodes, so an `Rc` would buy nothing and cost
/// the ability to write them as plain nested calls.
#[derive(Debug, Clone, PartialEq)]
pub enum Expr {
    /// A literal. The only place a number enters a field.
    Const(f32),
    /// Position components, in metres.
    X,
    Y,
    Z,
    /// Seconds since the simulation started. **Never a wall clock** — this is
    /// the tick count times the fixed timestep (never-do #8), and it is the
    /// reason a field is reproducible at all.
    T,
    Add(Box<Expr>, Box<Expr>),
    Sub(Box<Expr>, Box<Expr>),
    Mul(Box<Expr>, Box<Expr>),
    Div(Box<Expr>, Box<Expr>),
    Sin(Box<Expr>),
    Cos(Box<Expr>),
    Abs(Box<Expr>),
    Min(Box<Expr>, Box<Expr>),
    Max(Box<Expr>, Box<Expr>),
    /// An authored scalar, looked up by name.
    ///
    /// **The seam that makes a field authorable rather than baked.** The
    /// generated Slang reads it from a parameter block the scene fills, and
    /// the CPU reads it from the same values — so changing the wind in a
    /// `.loom` file changes both sides, exactly as editing the tree does.
    Param(&'static str),
    /// Value noise at a point ([`noise::value`]), in `[0, 1)`.
    ///
    /// A primitive rather than something assembled from the arithmetic above,
    /// because its lattice hash is integer and this tree is float. See the
    /// `noise` module for why that exception is worth making.
    Noise(Box<Expr>, Box<Expr>, Box<Expr>),
}

impl Expr {
    /// Evaluate on the CPU.
    ///
    /// The operation order here and the operation order in the emitted Slang
    /// are the same by construction, because both walk this tree. What can
    /// still differ is the hardware's `sin`, which is why the agreement test
    /// asserts an epsilon rather than equality.
    #[must_use]
    pub fn eval(&self, p: [f32; 3], t: f32) -> f32 {
        self.eval_with(p, t, &Params::DEFAULT)
    }

    /// Evaluate with authored parameters.
    ///
    /// An unknown name reads as zero rather than panicking: a field that loses
    /// a term is visible immediately, and stopping the simulation over a typo
    /// in a shader-side name helps nobody. [`Params::missing`] is what catches
    /// it, at load, where the message can name the field.
    #[must_use]
    pub fn eval_with(&self, p: [f32; 3], t: f32, params: &Params) -> f32 {
        match self {
            Self::Const(v) => *v,
            Self::X => p[0],
            Self::Y => p[1],
            Self::Z => p[2],
            Self::T => t,
            Self::Add(a, b) => a.eval_with(p, t, params) + b.eval_with(p, t, params),
            Self::Sub(a, b) => a.eval_with(p, t, params) - b.eval_with(p, t, params),
            Self::Mul(a, b) => a.eval_with(p, t, params) * b.eval_with(p, t, params),
            Self::Div(a, b) => a.eval_with(p, t, params) / b.eval_with(p, t, params),
            Self::Sin(a) => a.eval_with(p, t, params).sin(),
            Self::Cos(a) => a.eval_with(p, t, params).cos(),
            Self::Abs(a) => a.eval_with(p, t, params).abs(),
            Self::Min(a, b) => a.eval_with(p, t, params).min(b.eval_with(p, t, params)),
            Self::Max(a, b) => a.eval_with(p, t, params).max(b.eval_with(p, t, params)),
            Self::Param(name) => params.get(name),
            Self::Noise(x, y, z) => noise::value([
                x.eval_with(p, t, params),
                y.eval_with(p, t, params),
                z.eval_with(p, t, params),
            ]),
        }
    }

    /// Every parameter name this expression reads, in tree order.
    pub fn params(&self, out: &mut Vec<&'static str>) {
        match self {
            Self::Const(_) | Self::X | Self::Y | Self::Z | Self::T => {}
            Self::Param(name) => {
                if !out.contains(name) {
                    out.push(name);
                }
            }
            Self::Sin(a) | Self::Cos(a) | Self::Abs(a) => a.params(out),
            Self::Add(a, b)
            | Self::Sub(a, b)
            | Self::Mul(a, b)
            | Self::Div(a, b)
            | Self::Min(a, b)
            | Self::Max(a, b) => {
                a.params(out);
                b.params(out);
            }
            Self::Noise(x, y, z) => {
                x.params(out);
                y.params(out);
                z.params(out);
            }
        }
    }

    /// Emit the equivalent Slang expression.
    ///
    /// Fully parenthesised rather than precedence-aware. Precedence is a place
    /// to introduce a bug that only shows up as a subtly different field, and
    /// the shader compiler does not care how many brackets it reads.
    #[must_use]
    pub fn to_slang(&self) -> String {
        match self {
            // `{:?}` on an f32 always emits a decimal point or an exponent, so
            // `1.0` never reaches the shader as the integer `1` — which would
            // silently make a division integer division.
            Self::Const(v) => format!("{v:?}"),
            Self::X => "p.x".to_owned(),
            Self::Y => "p.y".to_owned(),
            Self::Z => "p.z".to_owned(),
            Self::T => "t".to_owned(),
            Self::Add(a, b) => format!("({} + {})", a.to_slang(), b.to_slang()),
            Self::Sub(a, b) => format!("({} - {})", a.to_slang(), b.to_slang()),
            Self::Mul(a, b) => format!("({} * {})", a.to_slang(), b.to_slang()),
            Self::Div(a, b) => format!("({} / {})", a.to_slang(), b.to_slang()),
            Self::Sin(a) => format!("sin({})", a.to_slang()),
            Self::Cos(a) => format!("cos({})", a.to_slang()),
            Self::Abs(a) => format!("abs({})", a.to_slang()),
            Self::Min(a, b) => format!("min({}, {})", a.to_slang(), b.to_slang()),
            Self::Max(a, b) => format!("max({}, {})", a.to_slang(), b.to_slang()),
            // Read from the block the caller passes in, so one compiled shader
            // serves every authored wind rather than one per scene.
            Self::Param(name) => format!("params.{name}"),
            Self::Noise(x, y, z) => format!(
                "loom_value_noise(float3({}, {}, {}))",
                x.to_slang(),
                y.to_slang(),
                z.to_slang()
            ),
        }
    }
}

/// Authored scalars a field reads by name.
///
/// A fixed-capacity list rather than a map: fields have a handful of
/// parameters, the order is the order the shader's block declares them, and a
/// `BTreeMap` here would be a heap allocation per sample in the simulation.
#[derive(Debug, Clone, PartialEq)]
pub struct Params {
    entries: Vec<(&'static str, f32)>,
}

impl Params {
    /// No parameters. Every `Param` reads zero.
    pub const DEFAULT: Self = Self { entries: Vec::new() };

    #[must_use]
    pub fn new(entries: &[(&'static str, f32)]) -> Self {
        Self { entries: entries.to_vec() }
    }

    #[must_use]
    pub fn get(&self, name: &str) -> f32 {
        self.entries.iter().find(|(key, _)| *key == name).map_or(0.0, |(_, v)| *v)
    }

    pub fn set(&mut self, name: &'static str, value: f32) {
        if let Some(slot) = self.entries.iter_mut().find(|(key, _)| *key == name) {
            slot.1 = value;
        } else {
            self.entries.push((name, value));
        }
    }

    #[must_use]
    pub fn entries(&self) -> &[(&'static str, f32)] {
        &self.entries
    }

    /// Names a field reads that these parameters do not carry.
    ///
    /// **The check that makes "unknown reads zero" safe.** A silent zero is
    /// the right runtime behaviour and the wrong thing to discover at runtime,
    /// so a caller asks this once and reports the names.
    #[must_use]
    pub fn missing(&self, field: &Field) -> Vec<&'static str> {
        let mut wanted = Vec::new();
        for axis in &field.body {
            axis.params(&mut wanted);
        }
        wanted
            .into_iter()
            .filter(|name| !self.entries.iter().any(|(key, _)| key == name))
            .collect()
    }
}

impl Default for Params {
    fn default() -> Self {
        Self::DEFAULT
    }
}

/// Convenience constructors, so a field reads like the maths rather than like
/// a tree being assembled.
#[must_use]
pub fn c(v: f32) -> Expr {
    Expr::Const(v)
}

impl std::ops::Add for Expr {
    type Output = Self;
    fn add(self, rhs: Self) -> Self {
        Self::Add(Box::new(self), Box::new(rhs))
    }
}

impl std::ops::Sub for Expr {
    type Output = Self;
    fn sub(self, rhs: Self) -> Self {
        Self::Sub(Box::new(self), Box::new(rhs))
    }
}

impl std::ops::Mul for Expr {
    type Output = Self;
    fn mul(self, rhs: Self) -> Self {
        Self::Mul(Box::new(self), Box::new(rhs))
    }
}

impl std::ops::Div for Expr {
    type Output = Self;
    fn div(self, rhs: Self) -> Self {
        Self::Div(Box::new(self), Box::new(rhs))
    }
}

#[must_use]
pub fn sin(a: Expr) -> Expr {
    Expr::Sin(Box::new(a))
}

#[must_use]
pub fn cos(a: Expr) -> Expr {
    Expr::Cos(Box::new(a))
}

#[must_use]
pub fn abs(a: Expr) -> Expr {
    Expr::Abs(Box::new(a))
}

#[must_use]
pub fn min(a: Expr, b: Expr) -> Expr {
    Expr::Min(Box::new(a), Box::new(b))
}

#[must_use]
pub fn max(a: Expr, b: Expr) -> Expr {
    Expr::Max(Box::new(a), Box::new(b))
}

/// A named three-component field over position and time.
#[derive(Debug, Clone)]
pub struct Field {
    pub name: &'static str,
    pub body: [Expr; 3],
}

impl Field {
    #[must_use]
    pub fn eval(&self, p: [f32; 3], t: f32) -> [f32; 3] {
        self.eval_with(p, t, &Params::DEFAULT)
    }

    /// Evaluate all three axes with authored parameters.
    #[must_use]
    pub fn eval_with(&self, p: [f32; 3], t: f32, params: &Params) -> [f32; 3] {
        [
            self.body[0].eval_with(p, t, params),
            self.body[1].eval_with(p, t, params),
            self.body[2].eval_with(p, t, params),
        ]
    }

    /// The Slang function, ready to be `#include`d.
    #[must_use]
    pub fn to_slang(&self) -> String {
        format!(
            "float3 {}(float3 p, float t, {} params) {{\n    return float3(\n        {},\n        {},\n        {});\n}}\n",
            self.name,
            self.params_struct_name(),
            self.body[0].to_slang(),
            self.body[1].to_slang(),
            self.body[2].to_slang(),
        )
    }

    /// The Slang struct holding this field's authored parameters.
    #[must_use]
    pub fn params_struct_name(&self) -> String {
        let mut name = String::new();
        let mut capitalise = true;
        for ch in self.name.trim_end_matches("_at").chars() {
            if ch == '_' {
                capitalise = true;
                continue;
            }
            if capitalise {
                name.extend(ch.to_uppercase());
                capitalise = false;
            } else {
                name.push(ch);
            }
        }
        format!("{name}Params")
    }

    /// The parameter names this field reads, in tree order.
    ///
    /// **Tree order, not sorted**, and that order is the struct's field order
    /// — so the Rust mirror and the shader block agree by construction rather
    /// than by two people keeping two lists in step. `scene.slang` has a long
    /// comment about the version of that hazard which already bit.
    #[must_use]
    pub fn params(&self) -> Vec<&'static str> {
        let mut out = Vec::new();
        for axis in &self.body {
            axis.params(&mut out);
        }
        out
    }

    /// The Slang declaration of this field's parameter block, and the reader
    /// that fills it from a flat array.
    ///
    /// **The reader is generated for the same reason the field is.** A caller
    /// has to get the parameters into that struct somehow, and the obvious
    /// way — a Rust array on one side and hand-written assignments on the
    /// other — is one order described twice. Emitting both from
    /// [`Self::params`] means the array index and the struct field are the
    /// same list, and `scene.slang`'s push block documents at length what
    /// happens when they are not.
    #[must_use]
    pub fn params_to_slang(&self) -> String {
        let struct_name = self.params_struct_name();
        let mut out = format!("struct {struct_name} {{\n");
        for name in self.params() {
            out.push_str(&format!("    float {name};\n"));
        }
        out.push_str("};\n\n");

        out.push_str(&format!("{struct_name} {}_params(float *v) {{\n", self.name));
        out.push_str(&format!("    {struct_name} params;\n"));
        for (index, name) in self.params().iter().enumerate() {
            out.push_str(&format!("    params.{name} = v[{index}];\n"));
        }
        out.push_str("    return params;\n}\n");
        out
    }

    /// This field's parameters as a flat array, in the order the generated
    /// Slang reader expects. The other half of the pairing above.
    #[must_use]
    pub fn params_array(&self, params: &Params) -> Vec<f32> {
        self.params().into_iter().map(|name| params.get(name)).collect()
    }
}

/// Every field both sides need. One list, so nothing can be generated for the
/// GPU that the CPU does not also have.
#[must_use]
pub fn all() -> Vec<Field> {
    vec![wind(), clouds()]
}

/// `min(max(e, 0), 1)`, because the tree has no saturate of its own.
fn saturate(e: Expr) -> Expr {
    Expr::Min(Box::new(Expr::Max(Box::new(e), Box::new(c(0.0)))), Box::new(c(1.0)))
}

/// `smoothstep(edge0, edge1, x)`, assembled from the primitives.
///
/// **The caller must guarantee `edge1 != edge0`.** There is no branch in an
/// expression tree, so a zero-width band divides by zero and produces a NaN
/// that propagates silently to every pixel. Every use below spaces the edges
/// with a constant.
fn smoothstep(edge0: Expr, edge1: Expr, x: Expr) -> Expr {
    let t = saturate((x - edge0.clone()) / (edge1 - edge0));
    t.clone() * t.clone() * (c(3.0) - c(2.0) * t)
}

/// Cloud cover overhead: 0 clear sky, 1 solid deck.
///
/// **ADR 0016's step 1, and the reason that ADR argues this shape fits.** Cloud
/// coverage is a scalar function of position and time, which is exactly what
/// `Expr` expresses — unlike grass placement (neighbourhood search) and
/// Gerstner waves (a loop with vector output), both of which needed hand-written
/// Slang twins. So the clouds the eye sees and the rain the simulation feels are
/// the same function *by construction*, not by discipline.
///
/// **Scalar in `body[0]`; the other two axes are zero.** A `Field` is a float3
/// because the wind is, and inventing a second field type to save eight bytes of
/// a register would be the abstraction this codebase keeps refusing. The sky
/// shader reads `.x`.
///
/// It advects along the P1 wind, so a squall crosses the sky the same way the
/// grass leans and the rain falls — coherence that costs nothing here and is
/// most of what sells weather.
#[must_use]
pub fn clouds() -> Field {
    let cover = Expr::Param("cloud_cover");
    let scale = Expr::Param("cloud_scale");
    let drift = Expr::Param("cloud_drift");
    // The same three names the wind field reads, on purpose: a scene authors
    // one wind and the deck moves with it.
    let dir_x = Expr::Param("dir_x");
    let dir_z = Expr::Param("dir_z");
    let speed = Expr::Param("speed");

    // Metres the deck has travelled. Subtracted from the sample position, so
    // the pattern moves *downwind* rather than the sampler moving upwind.
    let travel = speed * drift * Expr::T;
    let u = (Expr::X - dir_x * travel.clone()) / scale.clone();
    let v = (Expr::Z - dir_z * travel) / scale;

    // Three octaves. The fourth is below the resolution of a sky gradient and
    // costs a full noise evaluation per pixel to prove it.
    let n = fbm(u, c(0.0), v, 3);

    // **The coverage curve.** `cover` slides the threshold rather than scaling
    // the result, which is what makes partial cover look like separate clouds
    // instead of a uniform grey veil: at cover 0.3 only the top 30% of the noise
    // survives, and those are islands.
    //
    // The band is a constant so the divide above can never be by zero, and it
    // is wide enough that a cloud has a soft edge rather than a cut-out one.
    const BAND: f32 = 0.18;
    let threshold = c(1.0) - cover;
    let coverage = smoothstep(threshold.clone(), threshold + c(BAND), n);

    Field { name: "clouds_at", body: [coverage, c(0.0), c(0.0)] }
}

/// The parameters [`clouds`] reads, at their defaults.
///
/// **Half cover, because a scene that authors nothing should look like weather
/// rather than like a decision.** The same reasoning as [`wind_defaults`].
///
/// `cloud_scale` is metres per unit of noise: 1200 m puts a few cloud masses
/// across a horizon, which is what a cumulus field looks like from the ground.
/// `cloud_drift` is a multiplier on the wind because cloud-level wind is faster
/// than the wind at head height that `speed` describes.
#[must_use]
pub fn cloud_defaults() -> Params {
    Params::new(&[
        ("cloud_cover", 0.5),
        ("cloud_scale", 1200.0),
        ("cloud_drift", 2.5),
        ("dir_x", 1.0),
        ("dir_z", 0.0),
        ("speed", 5.5),
    ])
}

/// The wind field: a steady direction plus a few travelling gusts, thinning
/// toward the ground.
///
/// **Phase 1's field, landed early as S2's proof.** Its shape follows the wind
/// research: a base directional vector with two or three sinusoidal gust terms
/// modulating magnitude, and a height profile. Plain sinusoids rather than
/// curl noise, which the implementation order defers until particle advection
/// shows visible sinks.
///
/// The numbers are a placeholder for Phase 1 to author properly. What S2
/// proves is that whatever they become, both sides compute the same thing.
#[must_use]
pub fn wind() -> Field {
    // The authored direction, as a unit vector the caller supplies. Held as
    // two parameters rather than an angle because the shader would need the
    // trig anyway, and because a slew from one direction to another is a lerp
    // of vectors — an angle wraps at ±180° and snaps when it does, which is
    // exactly the artifact P1's exit criterion asks to bound.
    let dir_x = Expr::Param("dir_x");
    let dir_z = Expr::Param("dir_z");
    let speed = Expr::Param("speed");

    // A gust travelling *along* the wind: `sin(k·p − ωt)`. Subtracting time is
    // what makes the pattern move downwind rather than stand still and pulse,
    // which reads as the whole field breathing in unison — the single most
    // recognisable "this is fake" artifact in a vegetation shot.
    let gust = |k: f32, rate: f32, amp: Expr| {
        amp * sin(
            (Expr::X * c(k) * dir_x.clone()) + (Expr::Z * c(k) * dir_z.clone())
                - (Expr::T * c(rate)),
        )
    };

    // Three terms at spread-out wavelengths: one broad swell, one mid, one
    // quick. Two is visibly periodic, five is indistinguishable from three.
    //
    // **The amplitudes sum to 0.33, not 1.0.** The first version summed to one
    // and the CLI assertion caught what that means: the wind swings from dead
    // calm to double its mean. Meteorology quotes a gust factor — peak over
    // mean — of about 1.3 to 1.5 over open ground, so ±33% at `gustiness = 1`
    // is the honest default, and a squally 2.0 still only reaches ±66%.
    let gustiness = Expr::Param("gustiness");
    let strength = c(1.0)
        + gust(0.05, 0.45, gustiness.clone() * c(0.18))
        + gust(0.13, 0.9, gustiness.clone() * c(0.10))
        + gust(0.31, 1.7, gustiness * c(0.05));

    // One octave of fBm turbulence, advected downwind so it travels with the
    // air rather than sitting in world space like a texture. Deliberately one
    // octave: the implementation order defers curl noise until particle
    // advection shows visible sinks, and more octaves of value noise buy
    // detail nothing at this scale can see.
    let drift = Expr::T * c(0.08) * speed.clone();
    let turbulence = |seed: f32| {
        fbm(
            (Expr::X * c(0.06)) - (drift.clone() * dir_x.clone()) + c(seed),
            Expr::Y * c(0.06),
            (Expr::Z * c(0.06)) - (drift.clone() * dir_z.clone()) + c(seed),
            1,
        )
    };

    // **Approximating a power law, not a ramp.** Wind over open ground follows
    // roughly `(h/h_ref)^α` with α near 1/7: steep in the first metre, nearly
    // flat above a few. `pow` is not in the vocabulary — and would not be
    // worth adding for this — so the shape comes from `h / (h + 1)`, which is
    // the same qualitative curve: fast rise, smooth saturation, no knee.
    //
    // The first attempt was two linear segments and it was measurably wrong in
    // the place that matters: it put roughly half the correct wind at ankle
    // height, where grass lives, and reached free-stream by three metres.
    //
    // The `2.0` is fitted, not chosen — it is the value that minimises the
    // worst error against a 1/7 law at 0.2 m, 1.8 m and 5 m, and the test
    // checks all three against the real numbers rather than against whatever
    // this happens to produce.
    let above = max(c(0.0), Expr::Y);
    let drag = Expr::Param("ground_drag");
    let profile =
        drag.clone() + (c(1.0) - drag) * (above.clone() / (above + c(2.0)));

    let magnitude = speed * strength * profile.clone();
    let swirl = Expr::Param("turbulence");

    Field {
        name: "wind_at",
        body: [
            magnitude.clone() * dir_x.clone() + swirl.clone() * (turbulence(0.0) * c(2.0) - c(1.0)),
            // Vertical stirring, an order of magnitude smaller. Wind is not
            // flat, and vegetation that only ever leans sideways reads as a
            // row of flags rather than as a field in weather.
            swirl.clone() * (turbulence(11.7) * c(2.0) - c(1.0)) * c(0.35) * profile,
            magnitude * dir_z.clone() + swirl * (turbulence(23.4) * c(2.0) - c(1.0)),
        ],
    }
}

/// Fractional Brownian motion: octaves of [`Expr::Noise`], each half the
/// amplitude and twice the frequency of the last.
///
/// Built here as a tree rather than as a primitive, so it is generated for
/// both sides from one description — only the noise underneath it is written
/// twice, and that is exactly compared.
#[must_use]
pub fn fbm(x: Expr, y: Expr, z: Expr, octaves: u32) -> Expr {
    let mut total = c(0.0);
    let mut amplitude = 1.0_f32;
    let mut frequency = 1.0_f32;
    let mut normaliser = 0.0_f32;

    for _ in 0..octaves {
        total = total
            + c(amplitude)
                * Expr::Noise(
                    Box::new(x.clone() * c(frequency)),
                    Box::new(y.clone() * c(frequency)),
                    Box::new(z.clone() * c(frequency)),
                );
        normaliser += amplitude;
        amplitude *= 0.5;
        frequency *= 2.0;
    }

    // Normalised, so the result stays in [0, 1) whatever the octave count —
    // otherwise adding an octave quietly changes the field's magnitude and
    // every tuned number downstream of it.
    if normaliser > 0.0 { total * c(1.0 / normaliser) } else { total }
}

/// The parameters [`wind`] reads, at their defaults.
///
/// Beaufort 4 — a "moderate breeze", 5.5 m/s — blowing from the west. Chosen
/// because it is the lowest wind that visibly moves foliage, so a scene that
/// authors nothing still looks like weather rather than like a still.
#[must_use]
pub fn wind_defaults() -> Params {
    Params::new(&[
        ("dir_x", 1.0),
        ("dir_z", 0.0),
        ("speed", 5.5),
        ("gustiness", 1.0),
        ("turbulence", 0.8),
        // The fraction of free-stream wind still blowing at ground level.
        // 0.45 is not a taste call: it is what makes the curve above match a
        // 1/7 power law over the first few metres, which the height-profile
        // test checks against the real numbers.
        ("ground_drag", 0.45),
    ])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_constant_is_the_same_on_both_sides() {
        let e = c(2.5);

        assert!((e.eval([0.0; 3], 0.0) - 2.5).abs() < 1e-9);
        assert_eq!(e.to_slang(), "2.5");
    }

    /// **A whole number must not reach the shader as an integer.** Slang would
    /// read `1 / 2` as integer division and quietly produce zero, and the
    /// field would be wrong in a way no test of the Rust side could see.
    #[test]
    fn a_whole_number_keeps_its_decimal_point() {
        assert_eq!(c(1.0).to_slang(), "1.0");
        assert_eq!(c(-3.0).to_slang(), "-3.0");
        assert_eq!((c(1.0) / c(2.0)).to_slang(), "(1.0 / 2.0)");
    }

    /// Precedence is somewhere to introduce a bug that shows up only as a
    /// subtly different field, so the emitter does not rely on it.
    #[test]
    fn emitted_expressions_are_fully_parenthesised() {
        let e = (c(1.0) + c(2.0)) * c(3.0);

        assert_eq!(e.to_slang(), "((1.0 + 2.0) * 3.0)");
        assert!((e.eval([0.0; 3], 0.0) - 9.0).abs() < 1e-6);
    }

    #[test]
    fn position_and_time_reach_both_backends() {
        let e = Expr::X * Expr::Y + Expr::Z - Expr::T;

        assert!((e.eval([2.0, 3.0, 10.0], 4.0) - 12.0).abs() < 1e-6);
        assert_eq!(e.to_slang(), "(((p.x * p.y) + p.z) - t)");
    }

    #[test]
    fn every_operation_evaluates_and_emits() {
        let p = [0.5, -1.5, 2.0];
        let cases: [(Expr, f32, &str); 6] = [
            (sin(Expr::X), 0.5_f32.sin(), "sin(p.x)"),
            (cos(Expr::X), 0.5_f32.cos(), "cos(p.x)"),
            (abs(Expr::Y), 1.5, "abs(p.y)"),
            (min(Expr::X, Expr::Z), 0.5, "min(p.x, p.z)"),
            (max(Expr::X, Expr::Z), 2.0, "max(p.x, p.z)"),
            (Expr::Z / Expr::X, 4.0, "(p.z / p.x)"),
        ];
        for (expr, expected, slang) in cases {
            assert!(
                (expr.eval(p, 0.0) - expected).abs() < 1e-6,
                "{slang} evaluated to {}",
                expr.eval(p, 0.0)
            );
            assert_eq!(expr.to_slang(), slang);
        }
    }

    /// The generated function has to be a function, with the signature the
    /// shader calls. A field that emits a bare expression compiles nowhere.
    #[test]
    fn a_field_emits_a_callable_slang_function() {
        let slang = wind().to_slang();

        assert!(
            slang.starts_with("float3 wind_at(float3 p, float t, WindParams params) {"),
            "{slang}"
        );
        assert!(slang.trim_end().ends_with('}'), "{slang}");
        assert_eq!(slang.matches("return float3(").count(), 1);
    }

    /// **The wind has to actually vary.** A field that emits a constant would
    /// pass every agreement test ever written and animate nothing.
    #[test]
    fn the_wind_field_varies_in_space_and_in_time() {
        let field = wind();
        let p = wind_defaults();
        let here = field.eval_with([0.0, 2.0, 0.0], 0.0, &p);
        let later = field.eval_with([0.0, 2.0, 0.0], 3.0, &p);
        let over_there = field.eval_with([25.0, 2.0, 9.0], 0.0, &p);

        assert!(here != later, "the field does not move: {here:?}");
        assert!(here != over_there, "the field is uniform: {here:?}");
    }

    /// **Without parameters a field is inert, not wrong.** An unset `speed`
    /// reads zero, so the wind is calm rather than NaN or a panic — and
    /// `Params::missing` is what turns that into a message.
    #[test]
    fn an_unparameterised_field_is_calm_and_says_what_it_wanted() {
        let field = wind();

        let calm = field.eval([3.0, 2.0, 5.0], 1.0);
        assert_eq!(calm[0], 0.0);
        assert_eq!(calm[2], 0.0);

        let missing = Params::DEFAULT.missing(&field);
        assert!(missing.contains(&"speed"), "{missing:?}");
        assert!(missing.contains(&"dir_x"), "{missing:?}");
        assert!(
            wind_defaults().missing(&field).is_empty(),
            "the defaults must cover every parameter the field reads: {:?}",
            wind_defaults().missing(&field)
        );
    }

    /// The parameter block the shader declares must be exactly what the field
    /// reads, in the order the field reads it — two lists kept in step by
    /// hand is the layout-described-twice hazard `scene.slang` documents.
    #[test]
    fn the_parameter_block_matches_what_the_field_reads() {
        let field = wind();
        let declared = field.params_to_slang();

        for name in field.params() {
            assert!(declared.contains(&format!("float {name};")), "{declared}");
        }
        assert!(declared.starts_with("struct WindParams {"), "{declared}");
    }

    /// Wind thins toward the ground and does not invert below it.
    #[test]
    fn the_wind_slows_near_the_ground_and_never_reverses() {
        // Turbulence off, so this measures the height profile rather than
        // whichever way the noise happened to push at these points.
        let mut p = wind_defaults();
        p.set("turbulence", 0.0);
        let field = wind();
        let high = field.eval_with([1.0, 30.0, 1.0], 0.5, &p)[0];
        let low = field.eval_with([1.0, 0.0, 1.0], 0.5, &p)[0];
        let underground = field.eval_with([1.0, -50.0, 1.0], 0.5, &p)[0];

        assert!(high.abs() > low.abs(), "high {high} low {low}");
        assert!(
            underground.signum() == low.signum(),
            "the field flipped below the ground: {underground}"
        );
        // **Checked against the real law, not against a number I liked.**
        // A 1/7 power law referenced to 10 m gives 0.57 of free-stream at
        // 0.2 m and 0.78 at 1.8 m. The approximation has to land near those:
        // the first attempt was two linear segments and put roughly half the
        // correct wind at ankle height, which is exactly where grass lives.
        let reference = field.eval_with([1.0, 10.0, 1.0], 0.5, &p)[0].abs();
        for (height, expected) in [(0.2_f32, 0.57_f32), (1.8, 0.78), (5.0, 0.92)] {
            let got = field.eval_with([1.0, height, 1.0], 0.5, &p)[0].abs() / reference;
            assert!(
                (got - expected).abs() < 0.06,
                "at {height} m the profile gives {got} of free-stream, \
                 a 1/7 power law gives {expected}"
            );
        }
    }

    /// The list is what the generator walks. A field missing from it exists on
    /// the CPU and not on the GPU, which is the exact divergence S2 prevents.
    #[test]
    fn every_field_is_in_the_generated_list() {
        let names: Vec<&str> = all().iter().map(|f| f.name).collect();

        assert!(names.contains(&"wind_at"), "{names:?}");
    }
}

#[cfg(test)]
mod wind_exit {
    use super::*;

    /// The simulation's fixed timestep. Wind is sampled against tick counts,
    /// never a wall clock (never-do #8).
    const DT: f32 = 1.0 / 60.0;

    /// **P1's exit criterion: identical in debug and release across 10k
    /// ticks.** Run in both profiles by `cargo xtask validate`; here it pins
    /// the value so a change to the field, the noise or the profile shows up
    /// as a failing hash rather than as weather that quietly moved.
    ///
    /// Hashed rather than compared element-wise because the useful assertion
    /// is "all 30,000 samples are what they were", and one number says that.
    #[test]
    fn ten_thousand_ticks_hash_to_a_pinned_value() {
        let field = wind();
        let params = wind_defaults();

        let mut hash = 0xcbf2_9ce4_8422_2325_u64;
        for tick in 0..10_000_u32 {
            #[allow(clippy::cast_precision_loss)]
            let t = tick as f32 * DT;
            // Three fixed positions, spread over the range a scene uses.
            for p in [[0.0, 1.0, 0.0], [37.5, 0.25, -12.25], [-88.0, 14.0, 63.5]] {
                for component in field.eval_with(p, t, &params) {
                    // FNV-1a over the bits: a float compared by value would
                    // let a NaN through, and NaN is exactly the failure a
                    // determinism hash exists to catch.
                    for byte in component.to_bits().to_le_bytes() {
                        hash ^= u64::from(byte);
                        hash = hash.wrapping_mul(0x100_0000_01b3);
                    }
                }
            }
        }

        assert_eq!(
            hash, 0x413c_4a61_3c8c_eb4d,
            "the wind field changed. If that was deliberate, re-pin this \
             hash in the same commit — it is the only thing standing between \
             a retuned field and a silently different simulation."
        );
    }

    /// No sample is NaN or infinite. A single NaN poisons every downstream
    /// integration and the determinism hashes with it (format spec §1).
    #[test]
    fn the_field_is_finite_everywhere_it_is_sampled() {
        let field = wind();
        let params = wind_defaults();

        for tick in 0..2_000_u32 {
            #[allow(clippy::cast_precision_loss)]
            let t = tick as f32 * DT;
            #[allow(clippy::cast_precision_loss)]
            let p = [
                (tick as f32).mul_add(0.37, -400.0),
                (tick as f32).mul_add(-0.013, 20.0),
                (tick as f32).mul_add(0.53, -300.0),
            ];
            for component in field.eval_with(p, t, &params) {
                assert!(component.is_finite(), "at {p:?} t={t}: {component}");
            }
        }
    }

    /// **P1's other exit criterion: the direction slews, never snaps.**
    ///
    /// Asserted numerically because a snap is invisible in a still and only
    /// barely visible in motion — and it is the artifact that makes a wind
    /// field read as a lookup table rather than as air. The bound is per tick,
    /// so it is a rate rather than a total: over ten seconds the wind may turn
    /// as far as it likes, as long as it never jumps there.
    #[test]
    fn the_wind_direction_never_snaps() {
        let field = wind();
        let params = wind_defaults();
        let at = [12.0, 1.5, -7.0];

        let angle = |v: [f32; 3]| v[2].atan2(v[0]);
        let mut worst: f32 = 0.0;
        let mut previous = angle(field.eval_with(at, 0.0, &params));

        for tick in 1..6_000_u32 {
            #[allow(clippy::cast_precision_loss)]
            let t = tick as f32 * DT;
            let current = angle(field.eval_with(at, t, &params));
            // Shortest way round, so the ±π seam is not mistaken for a snap.
            let mut delta = current - previous;
            while delta > std::f32::consts::PI {
                delta -= std::f32::consts::TAU;
            }
            while delta < -std::f32::consts::PI {
                delta += std::f32::consts::TAU;
            }
            worst = worst.max(delta.abs());
            previous = current;
        }

        // Degrees per tick at 60 Hz. Three is a brisk but continuous swing —
        // about 180°/s at the very worst instant, and typically far less.
        let limit = 3.0_f32.to_radians();
        assert!(
            worst < limit,
            "the direction jumped {} degrees in one tick",
            worst.to_degrees()
        );
    }

    /// The magnitude is continuous too. A gust that steps rather than swells
    /// is the same artifact one axis over.
    #[test]
    fn the_wind_speed_never_steps() {
        let field = wind();
        let params = wind_defaults();
        let at = [3.0, 2.0, 4.0];

        let speed = |v: [f32; 3]| v[0].mul_add(v[0], v[2] * v[2]).sqrt();
        let mut worst: f32 = 0.0;
        let mut previous = speed(field.eval_with(at, 0.0, &params));

        for tick in 1..6_000_u32 {
            #[allow(clippy::cast_precision_loss)]
            let t = tick as f32 * DT;
            let current = speed(field.eval_with(at, t, &params));
            worst = worst.max((current - previous).abs());
            previous = current;
        }

        assert!(worst < 0.25, "the speed jumped by {worst} m/s in one tick");
    }

    /// **Gusts travel downwind rather than pulsing in place.** A field whose
    /// gust term does not carry the position would breathe in unison
    /// everywhere at once, which is the most recognisable "this is fake" tell
    /// in a vegetation shot.
    #[test]
    fn a_gust_arrives_downwind_later_than_upwind() {
        let field = wind();
        let mut params = wind_defaults();
        params.set("turbulence", 0.0);

        let speed_at = |x: f32, t: f32| {
            let v = field.eval_with([x, 2.0, 0.0], t, &params);
            v[0].mul_add(v[0], v[2] * v[2]).sqrt()
        };

        // The same instant, sampled far apart along the wind: the pattern is
        // in different places, so the readings differ.
        let upwind = speed_at(0.0, 4.0);
        let downwind = speed_at(60.0, 4.0);
        assert!(
            (upwind - downwind).abs() > 0.05,
            "the gust pattern is uniform in space: {upwind} vs {downwind}"
        );

        // And the pattern *moves*: the downwind point later resembles the
        // upwind point earlier, more than it resembles itself earlier.
        let series = |x: f32, shift: f32| {
            (0..200)
                .map(|i| {
                    #[allow(clippy::cast_precision_loss)]
                    let t = (i as f32).mul_add(0.05, shift);
                    speed_at(x, t)
                })
                .collect::<Vec<f32>>()
        };
        let reference = series(0.0, 0.0);
        let same_place_later = series(0.0, 6.0);
        let downwind_later = series(24.0, 6.0);

        let distance = |a: &[f32], b: &[f32]| {
            a.iter().zip(b).map(|(x, y)| (x - y).abs()).sum::<f32>()
        };
        assert!(
            distance(&reference, &downwind_later) < distance(&reference, &same_place_later),
            "the gust pattern does not travel with the wind"
        );
    }

    /// Turning the authored direction turns the field, and the two parameters
    /// are not silently swapped — an axis mix-up is invisible in a still and
    /// makes every downstream lean point the wrong way.
    #[test]
    fn the_authored_direction_steers_the_field() {
        let field = wind();

        let mut east = wind_defaults();
        east.set("turbulence", 0.0);
        let mut north = east.clone();
        north.set("dir_x", 0.0);
        north.set("dir_z", 1.0);

        let blowing_east = field.eval_with([5.0, 2.0, 5.0], 1.0, &east);
        let blowing_north = field.eval_with([5.0, 2.0, 5.0], 1.0, &north);

        assert!(blowing_east[0].abs() > blowing_east[2].abs() * 4.0, "{blowing_east:?}");
        assert!(blowing_north[2].abs() > blowing_north[0].abs() * 4.0, "{blowing_north:?}");
    }

    /// **The gust factor is a real number, not a taste call.** Peak over mean
    /// over open ground is about 1.3–1.5; the first version of this field
    /// summed its gust amplitudes to 1.0, which swings from dead calm to
    /// double and reads as a fan being switched on and off. A CLI assertion
    /// found it, not a unit test, which is why there is now a unit test.
    #[test]
    fn the_gust_factor_matches_open_ground_weather() {
        let field = wind();
        let mut params = wind_defaults();
        params.set("turbulence", 0.0);
        let at = [8.0, 4.0, -3.0];

        let mut low = f32::MAX;
        let mut high = f32::MIN;
        let mut total = 0.0_f64;
        let samples = 20_000_u32;
        for tick in 0..samples {
            #[allow(clippy::cast_precision_loss)]
            let t = tick as f32 * DT;
            let v = field.eval_with(at, t, &params);
            let speed = v[0].mul_add(v[0], v[2] * v[2]).sqrt();
            low = low.min(speed);
            high = high.max(speed);
            total += f64::from(speed);
        }

        #[allow(clippy::cast_possible_truncation)]
        let mean = (total / f64::from(samples)) as f32;
        let factor = high / mean;
        assert!(
            (1.2..1.6).contains(&factor),
            "gust factor {factor} (mean {mean}, peak {high}, lull {low}) — \
             open-ground weather is 1.3 to 1.5"
        );
        assert!(low > mean * 0.5, "the wind drops to a lull of {low} against a mean of {mean}");
    }

    /// Calm means calm. `speed = 0` has to give exactly no wind, because a
    /// scene that authors a still day and gets a breeze has no way to say what
    /// it meant.
    #[test]
    fn zero_speed_and_zero_turbulence_is_dead_calm() {
        let field = wind();
        let mut params = wind_defaults();
        params.set("speed", 0.0);
        params.set("turbulence", 0.0);

        for tick in 0..100_u32 {
            #[allow(clippy::cast_precision_loss)]
            let t = tick as f32 * DT;
            assert_eq!(field.eval_with([9.0, 3.0, 2.0], t, &params), [0.0, 0.0, 0.0]);
        }
    }
}
