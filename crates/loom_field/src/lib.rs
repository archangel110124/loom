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
        match self {
            Self::Const(v) => *v,
            Self::X => p[0],
            Self::Y => p[1],
            Self::Z => p[2],
            Self::T => t,
            Self::Add(a, b) => a.eval(p, t) + b.eval(p, t),
            Self::Sub(a, b) => a.eval(p, t) - b.eval(p, t),
            Self::Mul(a, b) => a.eval(p, t) * b.eval(p, t),
            Self::Div(a, b) => a.eval(p, t) / b.eval(p, t),
            Self::Sin(a) => a.eval(p, t).sin(),
            Self::Cos(a) => a.eval(p, t).cos(),
            Self::Abs(a) => a.eval(p, t).abs(),
            Self::Min(a, b) => a.eval(p, t).min(b.eval(p, t)),
            Self::Max(a, b) => a.eval(p, t).max(b.eval(p, t)),
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
        }
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
pub struct Field {
    pub name: &'static str,
    pub body: [Expr; 3],
}

impl Field {
    #[must_use]
    pub fn eval(&self, p: [f32; 3], t: f32) -> [f32; 3] {
        [
            self.body[0].eval(p, t),
            self.body[1].eval(p, t),
            self.body[2].eval(p, t),
        ]
    }

    /// The Slang function, ready to be `#include`d.
    #[must_use]
    pub fn to_slang(&self) -> String {
        format!(
            "float3 {}(float3 p, float t) {{\n    return float3(\n        {},\n        {},\n        {});\n}}\n",
            self.name,
            self.body[0].to_slang(),
            self.body[1].to_slang(),
            self.body[2].to_slang(),
        )
    }
}

/// Every field both sides need. One list, so nothing can be generated for the
/// GPU that the CPU does not also have.
#[must_use]
pub fn all() -> Vec<Field> {
    vec![wind()]
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
    // A gust travelling along the wind direction: `sin(k·p - ωt)`. Subtracting
    // time is what makes it travel *with* the wind rather than standing still
    // and pulsing, which reads as the whole field breathing in unison.
    let gust = |k: f32, speed: f32, amp: f32| {
        c(amp) * sin((Expr::X * c(k)) + (Expr::Z * c(k * 0.7)) - (Expr::T * c(speed)))
    };

    // Wind slows near the ground because the ground drags on it. Clamped so a
    // point below zero does not invert the field.
    let height = min(c(1.0), max(c(0.25), c(0.25) + Expr::Y * c(0.06)));

    let strength = c(1.0) + gust(0.18, 1.1, 0.35) + gust(0.07, 0.6, 0.22);

    Field {
        name: "wind_at",
        body: [
            c(6.0) * strength.clone() * height.clone(),
            // A little vertical stirring, an order of magnitude smaller: wind
            // is not flat, and grass that only ever leans sideways reads as a
            // flag rather than a field.
            c(0.6) * sin((Expr::X * c(0.11)) + (Expr::T * c(0.8))) * height.clone(),
            c(2.5) * strength * height,
        ],
    }
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

        assert!(slang.starts_with("float3 wind_at(float3 p, float t) {"), "{slang}");
        assert!(slang.trim_end().ends_with('}'), "{slang}");
        assert_eq!(slang.matches("return float3(").count(), 1);
    }

    /// **The wind has to actually vary.** A field that emits a constant would
    /// pass every agreement test ever written and animate nothing.
    #[test]
    fn the_wind_field_varies_in_space_and_in_time() {
        let field = wind();
        let here = field.eval([0.0, 2.0, 0.0], 0.0);
        let later = field.eval([0.0, 2.0, 0.0], 3.0);
        let over_there = field.eval([25.0, 2.0, 9.0], 0.0);

        assert!(here != later, "the field does not move: {here:?}");
        assert!(here != over_there, "the field is uniform: {here:?}");
    }

    /// Wind thins toward the ground and does not invert below it.
    #[test]
    fn the_wind_slows_near_the_ground_and_never_reverses() {
        let field = wind();
        let high = field.eval([1.0, 30.0, 1.0], 0.5)[0];
        let low = field.eval([1.0, 0.0, 1.0], 0.5)[0];
        let underground = field.eval([1.0, -50.0, 1.0], 0.5)[0];

        assert!(high.abs() > low.abs(), "high {high} low {low}");
        assert!(
            underground.signum() == low.signum(),
            "the field flipped below the ground: {underground}"
        );
    }

    /// The list is what the generator walks. A field missing from it exists on
    /// the CPU and not on the GPU, which is the exact divergence S2 prevents.
    #[test]
    fn every_field_is_in_the_generated_list() {
        let names: Vec<&str> = all().iter().map(|f| f.name).collect();

        assert!(names.contains(&"wind_at"), "{names:?}");
    }
}
