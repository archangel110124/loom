//! Type registry and schema-driven validation.
//!
//! Depends on nothing else in the workspace, and must stay that way (CLAUDE.md).

use std::collections::BTreeMap;

use schemars::{JsonSchema, Schema, SchemaGenerator};
use serde::Serialize;
use serde_json::Value;

/// Name → JSON Schema, for every component type the engine knows.
///
/// The schema *is* the registry entry: it already carries field names, types,
/// docs (from `///`), defaults, and range constraints. There is no second
/// hand-maintained description of a type to drift out of sync.
///
/// `BTreeMap`, not `HashMap` — iteration order is observable in `describe`
/// output and in generated schemas, and `clippy.toml` forbids `HashMap` for
/// exactly this reason.
#[derive(Debug, Default, Clone)]
pub struct TypeRegistry {
    types: BTreeMap<String, Schema>,
}

impl TypeRegistry {
    /// An empty registry. Call [`register`](Self::register) per component type.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Register `T` under `name`, deriving its schema.
    ///
    /// `name` is passed explicitly rather than taken from `T::schema_name()`
    /// because the scene format addresses components by the name authors write
    /// in `.loom` files, and that must not silently change when a Rust type is
    /// renamed. A rename is a format migration (`docs/format/README.md` §9).
    pub fn register<T: JsonSchema>(&mut self, name: &str) {
        let schema = SchemaGenerator::default().into_root_schema_for::<T>();
        self.types.insert(name.to_owned(), schema);
    }

    /// The schema for `name`, or `None` if nothing is registered under it.
    #[must_use]
    pub fn describe(&self, name: &str) -> Option<&Schema> {
        self.types.get(name)
    }

    /// Every registered type name, sorted.
    ///
    /// Sorted because it backs `list_types` and the "did you mean" list on a
    /// failed lookup, and unstable ordering there would make CLI output and
    /// golden tests flap. `BTreeMap` gives this for free.
    pub fn type_names(&self) -> impl Iterator<Item = &str> {
        self.types.keys().map(String::as_str)
    }

    /// Check a component's field values against its registered schema.
    ///
    /// Returns every violation, not just the first — an agent correcting one
    /// field at a time round-trips once per error, which is the retry loop
    /// `docs/format/README.md` §6 exists to avoid.
    ///
    /// The caller supplies the node path; this layer only knows types and
    /// fields. `loom_scene` wraps these into the full error shape.
    ///
    /// `ponytail:` ranges and enums. Patterns, nested objects and `required`
    /// are still unchecked — nothing declares them yet. Upgrade path when
    /// something does: swap this body for the `jsonschema` crate and map its
    /// output onto [`FieldError`], keeping the shape callers depend on.
    ///
    /// # Errors
    /// A [`FieldError`] per out-of-range field, or a single
    /// `unknown_component_type` if `type_name` is not registered.
    pub fn validate(&self, type_name: &str, value: &Value) -> Result<(), Vec<FieldError>> {
        let Some(schema) = self.describe(type_name) else {
            return Err(vec![FieldError {
                error: "unknown_component_type".to_owned(),
                field: type_name.to_owned(),
                value: Value::Null,
                constraint: String::new(),
                hint: None,
            }]);
        };

        let (Some(props), Some(fields)) = (
            schema.get("properties").and_then(Value::as_object),
            value.as_object(),
        ) else {
            return Ok(());
        };

        let mut errors = Vec::new();
        for (field, actual) in fields {
            let Some(field_schema) = props.get(field) else {
                // **Not `continue`.** Skipping unrecognised names is what made
                // this function a range check on correctly-spelled fields and
                // nothing more: `intensty = 400.0` validated clean and was
                // dropped at load, so the agent got `ok: true` and a dark room.
                //
                // Components have one to three fields, so listing them all *is*
                // the near-match — no edit distance needed, and it matches the
                // shape of the `unknown_component_type` hint.
                let known: Vec<&str> = props.keys().map(String::as_str).collect();
                errors.push(FieldError {
                    error: "unknown_field".to_owned(),
                    field: format!("{type_name}.{field}"),
                    value: actual.clone(),
                    constraint: format!("a field of {type_name}"),
                    hint: Some(format!("known fields: {}", known.join(", "))),
                });
                continue;
            };
            // The doc comment becomes the hint. A rejection message is the
            // agent's teacher (§6), so the docs do double duty.
            let hint = field_schema
                .get("description")
                .and_then(Value::as_str)
                .map(str::to_owned);

            // **Follow the reference first.** schemars puts an enum in
            // `$defs` and leaves a `$ref` where the field is, so everything
            // below — the declared type, the allowed values — is in the other
            // document. Walking the field schema as written meant an enum
            // carried no constraints at all and accepted anything.
            let field_schema = resolve(schema.as_value(), field_schema);

            // The declared type, checked before any range: a range check on a
            // value that is not a number silently passes, because `as_f64`
            // returns `None` and every comparison below is skipped.
            if let Some(mismatch) = type_mismatch(type_name, field, field_schema, actual) {
                errors.push(FieldError { hint, ..mismatch });
                continue;
            }

            // A closed set of names. The rejection lists them, because "not a
            // valid corner" without saying which corners exist is a message an
            // agent cannot act on (§6).
            if let Some(allowed) = field_schema.get("enum").and_then(Value::as_array)
                && !allowed.contains(actual)
            {
                let names: Vec<String> = allowed.iter().map(ToString::to_string).collect();
                errors.push(FieldError {
                    error: "field_not_in_enum".to_owned(),
                    field: format!("{type_name}.{field}"),
                    value: actual.clone(),
                    constraint: names.join(", "),
                    hint,
                });
                continue;
            }

            match actual {
                // Vec3-shaped fields carry their bounds on `items`, so a colour
                // channel written as 255 is caught per-channel rather than
                // slipping through because the field as a whole is an array.
                Value::Array(elements) => {
                    let (min, max) = bounds(field_schema.get("items"));
                    for (i, element) in elements.iter().enumerate() {
                        if let Some(n) = element.as_f64()
                            && out_of_range(n, min, max)
                        {
                            errors.push(FieldError {
                                error: "field_out_of_range".to_owned(),
                                field: format!("{type_name}.{field}[{i}]"),
                                value: element.clone(),
                                constraint: format_range(min, max),
                                hint: hint.clone(),
                            });
                        }
                    }
                }
                _ => {
                    let (min, max) = bounds(Some(field_schema));
                    if let Some(n) = actual.as_f64()
                        && out_of_range(n, min, max)
                    {
                        errors.push(FieldError {
                            error: "field_out_of_range".to_owned(),
                            field: format!("{type_name}.{field}"),
                            value: actual.clone(),
                            constraint: format_range(min, max),
                            hint,
                        });
                    }
                }
            }
        }

        if errors.is_empty() {
            Ok(())
        } else {
            Err(errors)
        }
    }
}

/// Follow a `$ref` into the schema's `$defs`, if the field is one.
///
/// Only local references, which is all schemars produces. Anything else is
/// returned unchanged rather than chased: a validator that fetches documents
/// is a different and much larger thing than this one.
fn resolve<'a>(root: &'a Value, field_schema: &'a Value) -> &'a Value {
    let Some(name) = field_schema
        .get("$ref")
        .and_then(Value::as_str)
        .and_then(|r| r.strip_prefix("#/$defs/"))
    else {
        return field_schema;
    };
    root.get("$defs")
        .and_then(|defs| defs.get(name))
        .unwrap_or(field_schema)
}

/// The declared type of `field`, and whether `actual` is one.
///
/// Returns the `field_type_mismatch` to report, or `None` when the value is
/// the right shape. Arity counts as shape: a two-element colour is not a
/// colour, and it is nastier than a plain type error because it looks right at
/// a glance and takes the whole component down with it at load.
///
/// `hint` is left empty and filled in by the caller, which has the field's doc
/// comment to hand.
fn type_mismatch(
    type_name: &str,
    field: &str,
    schema: &Value,
    actual: &Value,
) -> Option<FieldError> {
    let mismatch = |constraint: String| {
        Some(FieldError {
            error: "field_type_mismatch".to_owned(),
            field: format!("{type_name}.{field}"),
            value: actual.clone(),
            constraint,
            hint: None,
        })
    };

    match schema.get("type").and_then(Value::as_str) {
        // §7: integers coerce to floats where a float is expected, which
        // `as_f64` already does. Anything `as_f64` refuses is not a number —
        // including the null that a non-finite TOML float becomes, since JSON
        // has no way to carry NaN.
        Some("number" | "integer") if actual.as_f64().is_none() => {
            mismatch("number".to_owned())
        }
        Some("boolean") if !actual.is_boolean() => mismatch("boolean".to_owned()),
        Some("string") if !actual.is_string() => mismatch("string".to_owned()),
        Some("array") => {
            // Not `?` — that would report "no mismatch" for a value that is
            // not an array at all, which is exactly the `color = 5` case.
            let Some(elements) = actual.as_array() else {
                return mismatch("an array".to_owned());
            };
            let arity = |key| schema.get(key).and_then(Value::as_u64);
            let (low, high) = (arity("minItems"), arity("maxItems"));
            let n = elements.len() as u64;
            if low.is_some_and(|m| n < m) || high.is_some_and(|m| n > m) {
                return mismatch(match (low, high) {
                    (Some(a), Some(b)) if a == b => format!("an array of {a} numbers"),
                    (Some(a), Some(b)) => format!("an array of {a} to {b} numbers"),
                    (Some(a), None) => format!("an array of at least {a} numbers"),
                    (None, Some(b)) => format!("an array of at most {b} numbers"),
                    (None, None) => "an array".to_owned(),
                });
            }
            // Element type, so `[1.0, "x", 3.0]` is caught rather than being
            // skipped by the range check the way a non-number always was.
            let element_type = schema
                .get("items")
                .and_then(|i| i.get("type"))
                .and_then(Value::as_str);
            if matches!(element_type, Some("number" | "integer"))
                && let Some(i) = elements.iter().position(|e| e.as_f64().is_none())
            {
                return Some(FieldError {
                    error: "field_type_mismatch".to_owned(),
                    field: format!("{type_name}.{field}[{i}]"),
                    value: elements[i].clone(),
                    constraint: "number".to_owned(),
                    hint: None,
                });
            }
            None
        }
        _ => None,
    }
}

/// `minimum` / `maximum` off a schema node, if present.
fn bounds(schema: Option<&Value>) -> (Option<f64>, Option<f64>) {
    (
        schema.and_then(|s| s.get("minimum")).and_then(Value::as_f64),
        schema.and_then(|s| s.get("maximum")).and_then(Value::as_f64),
    )
}

fn out_of_range(n: f64, min: Option<f64>, max: Option<f64>) -> bool {
    min.is_some_and(|m| n < m) || max.is_some_and(|m| n > m)
}

/// A single field-level validation failure, shaped per `docs/format/README.md` §6.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct FieldError {
    /// Machine-readable code, e.g. `field_out_of_range`.
    pub error: String,
    /// `TypeName.field`.
    pub field: String,
    /// What was supplied.
    pub value: Value,
    /// Human- and agent-readable bound, e.g. `0.0..=10000.0`.
    pub constraint: String,
    /// Guidance from the field's doc comment, when it has one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hint: Option<String>,
}

/// Render bounds in Rust range syntax, matching the float rule in
/// `docs/format/README.md` §1 — always at least one fractional digit.
fn format_range(min: Option<f64>, max: Option<f64>) -> String {
    match (min, max) {
        (Some(lo), Some(hi)) => format!("{}..={}", fmt_f64(lo), fmt_f64(hi)),
        (Some(lo), None) => format!("{}..", fmt_f64(lo)),
        (None, Some(hi)) => format!("..={}", fmt_f64(hi)),
        (None, None) => String::new(),
    }
}

fn fmt_f64(v: f64) -> String {
    let s = format!("{v}");
    if s.contains(['.', 'e', 'E', 'N', 'i']) {
        s
    } else {
        format!("{s}.0")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use schemars::JsonSchema;
    use serde::{Deserialize, Serialize};

    /// A light source.
    #[derive(Serialize, Deserialize, JsonSchema)]
    struct Light {
        /// Luminous intensity. Interior lights are typically 100-800.
        #[schemars(range(min = 0.0, max = 10000.0))]
        intensity: f32,
        /// Linear RGB, each channel normalized 0..=1. Not 0-255.
        color: [f32; 3],
    }

    fn registry() -> TypeRegistry {
        let mut reg = TypeRegistry::new();
        reg.register::<Light>("Light");
        reg
    }

    /// **A typo must not be silent.** `docs/format/README.md` §6 makes
    /// `unknown_field` a normative M1 error code, and the whole point of
    /// schema validation as verification channel 1 is that an agent guessing
    /// `intensty` learns it guessed wrong. Skipping unrecognised names turned
    /// the validator into a range check on fields that happened to be spelled
    /// right.
    #[test]
    fn a_misspelled_field_is_rejected_and_the_near_match_is_offered() {
        let reg = registry();
        let value = serde_json::json!({ "intensty": 400.0 });

        let errs = reg
            .validate("Light", &value)
            .expect_err("`intensty` is not a field of Light");

        assert_eq!(errs.len(), 1);
        assert_eq!(errs[0].error, "unknown_field");
        assert_eq!(errs[0].field, "Light.intensty");
        // A rejection is the agent's teacher (§6) — naming the field it almost
        // spelled turns a retry loop into one correction.
        let hint = errs[0].hint.as_deref().unwrap_or_default();
        assert!(hint.contains("intensity"), "hint should suggest it: {hint}");
    }

    #[test]
    fn a_wrongly_typed_field_is_rejected() {
        let reg = registry();

        let errs = reg
            .validate("Light", &serde_json::json!({ "intensity": "very bright" }))
            .expect_err("a string is not a number");
        assert_eq!(errs[0].error, "field_type_mismatch");
        assert_eq!(errs[0].field, "Light.intensity");
        assert!(errs[0].constraint.contains("number"), "{:?}", errs[0].constraint);

        // The mirror case: a scalar where the schema declares an array.
        let errs = reg
            .validate("Light", &serde_json::json!({ "color": 5 }))
            .expect_err("a number is not a colour");
        assert_eq!(errs[0].error, "field_type_mismatch");
    }

    /// A two-element colour is not a colour. This one is nastier than a plain
    /// type error because the value looks right at a glance and the whole
    /// component is silently dropped at load.
    #[test]
    fn an_array_of_the_wrong_length_is_rejected() {
        let reg = registry();

        let errs = reg
            .validate("Light", &serde_json::json!({ "color": [1.0, 0.5] }))
            .expect_err("colour has three channels");

        assert_eq!(errs[0].error, "field_type_mismatch");
        assert_eq!(errs[0].field, "Light.color");
        assert!(errs[0].constraint.contains('3'), "{:?}", errs[0].constraint);
    }

    /// §1 of the format spec: non-finite floats "poison the determinism hashes
    /// M3 depends on". JSON cannot carry NaN, so this arrives as null from the
    /// TOML bridge — either way the field is not a number and must be refused
    /// rather than quietly replaced by the default.
    #[test]
    fn a_non_numeric_scalar_does_not_pass_as_a_number() {
        let reg = registry();

        let errs = reg
            .validate("Light", &serde_json::json!({ "intensity": null }))
            .expect_err("null is not an intensity");

        assert_eq!(errs[0].error, "field_type_mismatch");
    }

    /// Rejecting more must not start rejecting what was always legal.
    #[test]
    fn a_correct_component_still_validates() {
        let reg = registry();
        let value = serde_json::json!({ "intensity": 400.0, "color": [1.0, 0.5, 0.25] });

        assert!(reg.validate("Light", &value).is_ok());
        // Integers coerce to floats where a float is expected (§7).
        assert!(
            reg.validate("Light", &serde_json::json!({ "intensity": 400 }))
                .is_ok(),
            "§7: integers coerce to floats"
        );
        // A field simply being absent is not an error — defaults apply.
        assert!(reg.validate("Light", &serde_json::json!({})).is_ok());
    }

    #[test]
    fn describe_finds_a_registered_type_by_name() {
        let reg = registry();

        assert!(reg.describe("Light").is_some(), "Light was registered");
        assert!(reg.describe("Nonexistent").is_none());
    }

    /// The M1 exit criterion: the rejection must name the field, the value,
    /// AND the constraint. An error that says only "invalid scene" fails this.
    #[test]
    fn out_of_range_field_names_field_value_and_constraint() {
        let reg = registry();
        let value = serde_json::json!({ "intensity": 40000.0 });

        let errs = reg
            .validate("Light", &value)
            .expect_err("40000 is above the declared max of 10000");

        assert_eq!(errs.len(), 1, "exactly one field is out of range");
        let e = &errs[0];
        assert_eq!(e.error, "field_out_of_range");
        assert_eq!(e.field, "Light.intensity");
        assert_eq!(e.value, serde_json::json!(40000.0));
        assert_eq!(e.constraint, "0.0..=10000.0");
    }

    #[test]
    fn in_range_field_validates() {
        let reg = registry();
        let value = serde_json::json!({ "intensity": 420.0 });

        assert!(reg.validate("Light", &value).is_ok());
    }

    /// The doc comment on a field becomes the rejection's hint, so writing
    /// good docs is the same act as teaching the agent (§6).
    #[test]
    fn rejection_carries_the_field_doc_comment_as_a_hint() {
        let reg = registry();
        let value = serde_json::json!({ "intensity": 40000.0 });

        let errs = reg.validate("Light", &value).unwrap_err();

        let hint = errs[0].hint.as_deref().expect("field has a doc comment");
        assert!(
            hint.contains("Interior lights are typically 100-800"),
            "hint should carry the doc comment, got: {hint}"
        );
    }

    /// A component with an enum field, which is what `Hud.anchor` is.
    #[derive(Serialize, Deserialize, JsonSchema)]
    #[serde(rename_all = "snake_case")]
    enum Corner {
        TopLeft,
        BottomRight,
    }

    /// Something pinned to a corner of the screen.
    #[derive(Serialize, Deserialize, JsonSchema)]
    struct Pinned {
        /// Which corner it is measured from.
        corner: Corner,
    }

    /// **A misspelled enum value validated clean.** schemars puts an enum
    /// behind a `$ref` into `$defs`, and this walked the field schema without
    /// following it — so `anchor = "top_lefft"` was accepted, dropped at load,
    /// and the overlay silently drew in the wrong corner. Exactly the failure
    /// `unknown_field` exists to prevent, one level deeper.
    #[test]
    fn a_value_outside_an_enum_is_rejected() {
        let mut reg = TypeRegistry::new();
        reg.register::<Pinned>("Pinned");

        let errors = reg
            .validate("Pinned", &serde_json::json!({ "corner": "top_lefft" }))
            .expect_err("that is not a corner");

        assert_eq!(errors[0].error, "field_not_in_enum");
        assert_eq!(errors[0].field, "Pinned.corner");
        assert!(
            errors[0].constraint.contains("top_left"),
            "the rejection must list what is allowed: {}",
            errors[0].constraint
        );
    }

    #[test]
    fn a_valid_enum_value_still_passes() {
        let mut reg = TypeRegistry::new();
        reg.register::<Pinned>("Pinned");

        assert!(reg
            .validate("Pinned", &serde_json::json!({ "corner": "bottom_right" }))
            .is_ok());
    }

    /// A number where a name belongs is a different mistake and gets its own
    /// error, rather than being reported as "not one of these strings".
    #[test]
    fn a_number_where_an_enum_belongs_is_a_type_error() {
        let mut reg = TypeRegistry::new();
        reg.register::<Pinned>("Pinned");

        let errors = reg
            .validate("Pinned", &serde_json::json!({ "corner": 3 }))
            .expect_err("a corner is not a number");

        assert_eq!(errors[0].error, "field_type_mismatch", "{errors:?}");
    }
}
