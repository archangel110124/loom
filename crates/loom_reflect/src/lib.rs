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
    /// `ponytail:` range constraints only. Patterns, enums, nested objects, and
    /// `required` are not checked yet — the six M1 components use none of them.
    /// Upgrade path when they do: swap this body for the `jsonschema` crate and
    /// map its output onto [`FieldError`], keeping the shape callers depend on.
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
                continue;
            };
            // The doc comment becomes the hint. A rejection message is the
            // agent's teacher (§6), so the docs do double duty.
            let hint = field_schema
                .get("description")
                .and_then(Value::as_str)
                .map(str::to_owned);

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
    }

    fn registry() -> TypeRegistry {
        let mut reg = TypeRegistry::new();
        reg.register::<Light>("Light");
        reg
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
}

