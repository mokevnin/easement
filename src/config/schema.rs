//! The JSON Schema of `easement.yml` that `easement schema` prints.

use schemars::{Schema, SchemaGenerator, json_schema};

use super::Config;
use crate::check::RuleId;

/// Where editors find the schema that `easement schema` prints.
const SCHEMA_ID: &str =
    "https://raw.githubusercontent.com/mokevnin/easement/main/easement.schema.json";

/// The JSON Schema of `easement.yml`, for editors: the config types with their
/// doc comments as descriptions.
pub fn schema() -> Schema {
    let mut settings = schemars::generate::SchemaSettings::draft07();
    settings
        .transforms
        .push(Box::new(schemars::transform::RecursiveTransform(
            unwrap_lines,
        )));
    settings.into_generator().into_root_schema_for::<Config>()
}

/// The schema as `easement schema` prints it and the repository keeps it in
/// `easement.schema.json`.
pub fn schema_text() -> String {
    serde_json::to_string_pretty(&schema()).expect("a schema is JSON")
}

/// A doc comment keeps its line breaks; an editor shows a description as one
/// paragraph, so only a blank line stays a break.
fn unwrap_lines(schema: &mut Schema) {
    if let Some(serde_json::Value::String(text)) = schema.get_mut("description") {
        *text = text
            .split("\n\n")
            .map(|paragraph| paragraph.replace('\n', " "))
            .collect::<Vec<_>>()
            .join("\n\n");
    }
}

/// What the file holds besides the fields of [`Config`]: `extends`, which is
/// merged away before the config is read, and the free `x-` keys for anchors. A
/// file that `extends` another may leave out even `version` and `components`,
/// so the top level requires nothing.
pub(super) fn config_schema_extras(schema: &mut Schema) {
    schema.insert("$id".into(), SCHEMA_ID.into());
    schema.insert("title".into(), "easement.yml".into());
    schema.remove("required");
    schema.insert(
        "patternProperties".into(),
        serde_json::json!({ "^x-": {
            "description": "Free keys that only hold YAML anchors: `x-ban: &ban {...}`, then `<<: *ban` in a rule."
        }}),
    );
    if let Some(properties) = schema
        .get_mut("properties")
        .and_then(serde_json::Value::as_object_mut)
    {
        properties.insert(
            "extends".into(),
            serde_json::json!({
                "description": "Config files merged underneath this one, relative to this file. Patterns stay relative to the root of the config being loaded.",
                "anyOf": [
                    { "type": "string" },
                    { "type": "array", "items": { "type": "string" } }
                ]
            }),
        );
    }
}

/// `warn` takes the rules about the code and `unused-allowance`, not the
/// mistakes in the config.
pub(super) fn warn_schema(_: &mut SchemaGenerator) -> Schema {
    let rules: Vec<&str> = RuleId::ALL
        .into_iter()
        .filter(|rule| rule.warnable())
        .map(RuleId::as_str)
        .collect();
    json_schema!({
        "type": "array",
        "items": { "enum": rules }
    })
}

#[cfg(test)]
mod tests {
    /// Editors read `easement.schema.json` from the repository, so it must be
    /// the schema the config types give.
    #[test]
    fn the_schema_file_is_the_generated_schema() {
        let file = include_str!("../../easement.schema.json");
        assert!(
            file.trim_end() == super::schema_text(),
            "easement.schema.json is out of date: run `cargo run -q -- schema > easement.schema.json`"
        );
    }
}
