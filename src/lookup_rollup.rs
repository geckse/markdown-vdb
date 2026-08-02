//! Relation-backed Lookup and Rollup computed fields.
//!
//! This module deliberately shares the host-controlled patch path used by
//! Formula. Relation traversal and aggregation operate only on an immutable
//! index snapshot; filesystem access remains confined to the module runner.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::str::FromStr;

use serde_json::{Map as JsonMap, Value as JsonValue};
use sha2::{Digest, Sha256};

use super::{
    DerivedFieldPatch, Module, ModuleContext, ModuleDescriptor, ModuleDiagnostic, ModuleEvent,
    ModuleExecution,
};
use crate::formula::{FormulaDefinition, FormulaEngine};
use crate::index::types::{
    ComputedDependencyPathState, ComputedDependencySnapshot, ComputedFieldDiagnostic,
    ComputedFieldEntry, StoredFile,
};
use crate::schema::{FieldType, FormulaResultType, OverlaySchema, RelationDirection, Schema};

pub const LOOKUP_ROLLUP_MODULE_ID: &str = "lookup_rollup";
const MAX_DEPENDENCY_DEPTH: usize = 256;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DefinitionKind {
    Lookup,
    Rollup,
}

#[derive(Debug, Clone)]
struct Definition {
    kind: DefinitionKind,
    relation_field: String,
    target_field: String,
    direction: RelationDirection,
    relation_scope: Option<String>,
    formula: Option<String>,
    result_type: Option<FormulaResultType>,
    fingerprint: String,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
struct NodeKey {
    path: String,
    field: String,
}

impl NodeKey {
    fn new(path: impl Into<String>, field: impl Into<String>) -> Self {
        Self {
            path: path.into(),
            field: field.into(),
        }
    }
}

#[derive(Debug, Clone)]
struct EvaluationFailure {
    code: String,
    message: String,
    span_start: Option<usize>,
    span_end: Option<usize>,
}

impl EvaluationFailure {
    fn new(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            code: code.into(),
            message: message.into(),
            span_start: None,
            span_end: None,
        }
    }

    fn dependency(path: &str, field: &str, cause: &Self) -> Self {
        Self::new(
            "dependency_failed",
            format!(
                "computed target `{path}.{field}` failed ({}): {}",
                cause.code, cause.message
            ),
        )
    }
}

#[derive(Debug, Clone)]
enum EvaluationOutcome {
    Value(JsonValue),
    Error(EvaluationFailure),
}

#[derive(Debug)]
enum RelationSelection {
    Missing,
    Scalar(String),
    List(Vec<String>),
}

enum PreparedDefinition {
    Lookup(JsonValue),
    Rollup(Vec<JsonValue>),
}

#[derive(Debug, Default)]
pub struct LookupRollupModule {
    engine: FormulaEngine,
}

impl LookupRollupModule {
    fn definitions_for_path(overlay: &OverlaySchema, path: &str) -> BTreeMap<String, Definition> {
        let mut definitions = BTreeMap::new();
        for (name, field) in Schema::resolve_overlay_for_path(overlay, Some(path)) {
            let kind = match field.field_type.as_deref().map(str::to_ascii_lowercase) {
                Some(kind) if kind == "lookup" => DefinitionKind::Lookup,
                Some(kind) if kind == "rollup" => DefinitionKind::Rollup,
                _ => continue,
            };
            let Some(relation_field) = field
                .relation_field
                .as_deref()
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(str::to_string)
            else {
                continue;
            };
            let Some(target_field) = field
                .target_field
                .as_deref()
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(str::to_string)
            else {
                continue;
            };
            let direction = if kind == DefinitionKind::Lookup {
                RelationDirection::Outgoing
            } else {
                field
                    .relation_direction
                    .as_deref()
                    .and_then(crate::schema::parse_relation_direction_str)
                    .unwrap_or(RelationDirection::Outgoing)
            };
            let relation_scope = field
                .relation_scope
                .as_deref()
                .map(|scope| scope.trim().trim_matches('/').to_string())
                .filter(|scope| !scope.is_empty());
            let formula = (kind == DefinitionKind::Rollup)
                .then(|| field.formula.clone())
                .flatten();
            let result_type = (kind == DefinitionKind::Rollup)
                .then(|| {
                    field
                        .result_type
                        .as_deref()
                        .and_then(|value| FormulaResultType::from_str(value).ok())
                })
                .flatten();
            let fingerprint = definition_fingerprint(
                kind,
                &relation_field,
                &target_field,
                direction,
                relation_scope.as_deref(),
                formula.as_deref(),
                result_type,
            );
            definitions.insert(
                name,
                Definition {
                    kind,
                    relation_field,
                    target_field,
                    direction,
                    relation_scope,
                    formula,
                    result_type,
                    fingerprint,
                },
            );
        }
        definitions
    }

    fn collect_definitions(
        overlay: &OverlaySchema,
        files: &HashMap<String, StoredFile>,
    ) -> BTreeMap<NodeKey, Definition> {
        let mut result = BTreeMap::new();
        let mut paths: Vec<&String> = files.keys().collect();
        paths.sort();
        for path in paths {
            for (field, definition) in Self::definitions_for_path(overlay, path) {
                result.insert(NodeKey::new(path, field), definition);
            }
        }
        result
    }

    fn stored_diagnostic(field: &str, failure: &EvaluationFailure) -> ComputedFieldDiagnostic {
        ComputedFieldDiagnostic {
            module: LOOKUP_ROLLUP_MODULE_ID.to_string(),
            field: field.to_string(),
            code: failure.code.clone(),
            message: failure.message.clone(),
            span_start: failure.span_start,
            span_end: failure.span_end,
        }
    }

    fn report_diagnostic(path: &str, field: &str, failure: &EvaluationFailure) -> ModuleDiagnostic {
        ModuleDiagnostic {
            path: Some(path.to_string()),
            module: LOOKUP_ROLLUP_MODULE_ID.to_string(),
            field: field.to_string(),
            code: failure.code.clone(),
            message: failure.message.clone(),
            span_start: failure.span_start,
            span_end: failure.span_end,
        }
    }

    fn materialized_paths(
        event: &ModuleEvent,
        files: &HashMap<String, StoredFile>,
    ) -> BTreeSet<String> {
        match event {
            ModuleEvent::ManualRun { .. } | ModuleEvent::ManualPaths { .. } => {
                super::event_paths(event, files).into_iter().collect()
            }
            // Correctness is more important than an incremental shortcut here:
            // a change can invalidate any reverse lookup or rollup owner.
            _ => files.keys().cloned().collect(),
        }
    }

    /// Return the fields for which applying a candidate patch would change
    /// either module bookkeeping or the semantic frontmatter value.
    ///
    /// The module intentionally recomputes the complete collection for
    /// dependency correctness, but an identical result must converge without
    /// another source/index write.
    fn changed_patch_fields(
        file: &StoredFile,
        fields: &HashMap<String, ComputedFieldEntry>,
        frontmatter_set: &BTreeMap<String, JsonValue>,
        frontmatter_unset: &BTreeSet<String>,
    ) -> BTreeSet<String> {
        let mut candidates = frontmatter_unset.clone();
        candidates.extend(frontmatter_set.keys().cloned());
        candidates.extend(
            file.computed_fields
                .iter()
                .filter(|(_, entry)| entry.module == LOOKUP_ROLLUP_MODULE_ID)
                .map(|(field, _)| field.clone()),
        );
        candidates.extend(
            fields
                .iter()
                .filter(|(_, entry)| entry.module == LOOKUP_ROLLUP_MODULE_ID)
                .map(|(field, _)| field.clone()),
        );

        let current_frontmatter = match file.frontmatter.as_deref() {
            None => Some(JsonMap::new()),
            Some(raw) => serde_json::from_str::<JsonValue>(raw)
                .ok()
                .and_then(|value| value.as_object().cloned()),
        };

        candidates
            .into_iter()
            .filter(|field| {
                if file.computed_fields.get(field) != fields.get(field) {
                    return true;
                }
                let Some(current) = current_frontmatter.as_ref() else {
                    // A malformed/non-object snapshot should still reach the
                    // writer so it can emit the authoritative parse diagnostic.
                    return frontmatter_set.contains_key(field)
                        || frontmatter_unset.contains(field);
                };
                if let Some(value) = frontmatter_set.get(field) {
                    current.get(field) != Some(value)
                } else {
                    frontmatter_unset.contains(field) && current.contains_key(field)
                }
            })
            .collect()
    }

    fn materialization_matches(file: &StoredFile, field: &str, entry: &ComputedFieldEntry) -> bool {
        let Some(frontmatter) = file
            .frontmatter
            .as_deref()
            .and_then(|raw| serde_json::from_str::<JsonValue>(raw).ok())
            .and_then(|value| value.as_object().cloned())
        else {
            return false;
        };

        match (&entry.value_json, &entry.diagnostic) {
            (Some(_), None) => file.materialized_field_matches(field, entry),
            (None, Some(_)) => !entry.has_materialized_proof() && !frontmatter.contains_key(field),
            _ => false,
        }
    }

    fn build_execution(
        &self,
        context: &ModuleContext<'_>,
        event: &ModuleEvent,
        overlay: &OverlaySchema,
        schema_fingerprint: &str,
    ) -> ModuleExecution {
        let definitions = Self::collect_definitions(overlay, context.files);
        let mut evaluator =
            RelationEvaluator::new(context.files, overlay, &definitions, &self.engine);
        let materialized_paths = Self::materialized_paths(event, context.files);
        let mut execution = ModuleExecution {
            expected_schema_fingerprint: Some(schema_fingerprint.to_string()),
            ..ModuleExecution::default()
        };
        for definition in definitions.values().filter(|definition| {
            definition.kind == DefinitionKind::Rollup
                && definition.direction == RelationDirection::Incoming
        }) {
            if let Some(scope) = &definition.relation_scope {
                execution
                    .expected_incoming_scope_membership
                    .entry(scope.clone())
                    .or_insert_with(|| {
                        context
                            .files
                            .keys()
                            .filter(|path| crate::path_util::path_is_in_scope(path, scope))
                            .cloned()
                            .collect()
                    });
            }
        }

        for path in materialized_paths {
            let Some(file) = context.files.get(&path) else {
                continue;
            };
            let definitions_for_file: Vec<(NodeKey, Definition)> = definitions
                .range(NodeKey::new(&path, "")..)
                .take_while(|(key, _)| key.path == path)
                .map(|(key, definition)| (key.clone(), definition.clone()))
                .collect();
            let previous_fields: BTreeSet<String> = file
                .computed_fields
                .iter()
                .filter(|(_, entry)| entry.module == LOOKUP_ROLLUP_MODULE_ID)
                .map(|(field, _)| field.clone())
                .collect();
            execution.dependency_owners.insert(
                path.clone(),
                previous_fields
                    .iter()
                    .filter(|field| {
                        file.computed_fields
                            .get(*field)
                            .is_some_and(|entry| file.materialized_field_matches(field, entry))
                    })
                    .cloned()
                    .chain(
                        definitions_for_file
                            .iter()
                            .map(|(key, _)| key.field.clone()),
                    )
                    .collect(),
            );
            if definitions_for_file.is_empty() && previous_fields.is_empty() {
                continue;
            }

            execution.files_evaluated += 1;
            let mut entries = file.computed_fields.clone();
            entries.retain(|_, entry| entry.module != LOOKUP_ROLLUP_MODULE_ID);
            let mut frontmatter_unset: BTreeSet<String> = previous_fields
                .iter()
                .filter(|field| {
                    file.computed_fields
                        .get(*field)
                        .is_some_and(|entry| file.materialized_field_matches(field, entry))
                })
                .cloned()
                .collect();
            let mut frontmatter_set = BTreeMap::new();

            for (key, definition) in definitions_for_file {
                frontmatter_unset.insert(key.field.clone());
                let outcome = evaluator.evaluate_node(&key);
                let input_fingerprint = evaluator.input_fingerprint(&key, &definition.fingerprint);
                let incoming_scope = (definition.direction == RelationDirection::Incoming)
                    .then_some(definition.relation_scope.as_deref())
                    .flatten();
                let dependency_snapshot = evaluator.dependency_snapshot(&key, incoming_scope);
                for dependency_path in evaluator.dependency_paths(&key) {
                    if let Some(dependency) = context.files.get(dependency_path) {
                        execution
                            .expected_dependency_hashes
                            .insert(dependency_path.clone(), dependency.content_hash.clone());
                    }
                }
                if let Some(previous) = file.computed_fields.get(&key.field).filter(|entry| {
                    entry.module == LOOKUP_ROLLUP_MODULE_ID
                        && entry.definition_fingerprint == definition.fingerprint
                        && entry.input_fingerprint.as_deref() == Some(&input_fingerprint)
                        && !entry.dependency_snapshot.paths.is_empty()
                        && Self::materialization_matches(file, &key.field, entry)
                }) {
                    if let Some(diagnostic) = &previous.diagnostic {
                        execution.diagnostics.push(ModuleDiagnostic {
                            path: Some(path.clone()),
                            module: diagnostic.module.clone(),
                            field: key.field.clone(),
                            code: diagnostic.code.clone(),
                            message: diagnostic.message.clone(),
                            span_start: diagnostic.span_start,
                            span_end: diagnostic.span_end,
                        });
                    }
                    // Keep the snapshot that produced the cached outcome. A
                    // target's unrelated frontmatter edit may change its full
                    // content hash without changing the selected input or its
                    // fingerprint; refreshing only the provenance in that case
                    // would create a spurious computed-field update.
                    entries.insert(key.field.clone(), previous.clone());
                    frontmatter_unset.remove(&key.field);
                    continue;
                }

                let input_fingerprint = Some(input_fingerprint);
                match outcome {
                    EvaluationOutcome::Value(value) => {
                        let value_json = serde_json::to_string(&value).ok();
                        let materialized_value_json = value_json.clone();
                        frontmatter_set.insert(key.field.clone(), value);
                        entries.insert(
                            key.field.clone(),
                            ComputedFieldEntry {
                                module: LOOKUP_ROLLUP_MODULE_ID.to_string(),
                                definition_fingerprint: definition.fingerprint,
                                input_fingerprint,
                                dependency_snapshot,
                                value_json,
                                materialized_value_json,
                                diagnostic: None,
                            },
                        );
                    }
                    EvaluationOutcome::Error(failure) => {
                        execution
                            .diagnostics
                            .push(Self::report_diagnostic(&path, &key.field, &failure));
                        entries.insert(
                            key.field.clone(),
                            ComputedFieldEntry {
                                module: LOOKUP_ROLLUP_MODULE_ID.to_string(),
                                definition_fingerprint: definition.fingerprint,
                                input_fingerprint,
                                dependency_snapshot,
                                value_json: None,
                                materialized_value_json: None,
                                diagnostic: Some(Self::stored_diagnostic(&key.field, &failure)),
                            },
                        );
                    }
                }
            }

            let changed_fields =
                Self::changed_patch_fields(file, &entries, &frontmatter_set, &frontmatter_unset);
            if !changed_fields.is_empty() {
                execution.fields_updated += changed_fields.len();
                execution.derived_field_patches.push(DerivedFieldPatch {
                    path,
                    expected_content_hash: file.content_hash.clone(),
                    fields: entries,
                    frontmatter_set,
                    frontmatter_unset,
                });
            }
        }

        execution.expected_missing_dependency_states = evaluator.missing_dependency_states.clone();

        Self::finish_schema_patch(context, Some(overlay), &mut execution);
        execution
    }

    fn clear_owned_state(
        context: &ModuleContext<'_>,
        event: &ModuleEvent,
        code: &str,
        message: &str,
        execution: &mut ModuleExecution,
    ) {
        let paths = Self::materialized_paths(event, context.files);
        for path in paths {
            let file = &context.files[&path];
            let previous_fields: Vec<String> = file
                .computed_fields
                .iter()
                .filter(|(_, entry)| entry.module == LOOKUP_ROLLUP_MODULE_ID)
                .map(|(field, _)| field.clone())
                .collect();
            if previous_fields.is_empty() {
                continue;
            }
            execution.files_evaluated += 1;
            let mut entries = file.computed_fields.clone();
            entries.retain(|_, entry| entry.module != LOOKUP_ROLLUP_MODULE_ID);
            for field in &previous_fields {
                let failure = EvaluationFailure::new(code, message);
                entries.insert(
                    field.clone(),
                    ComputedFieldEntry {
                        module: LOOKUP_ROLLUP_MODULE_ID.to_string(),
                        definition_fingerprint: String::new(),
                        input_fingerprint: None,
                        dependency_snapshot: ComputedDependencySnapshot::default(),
                        value_json: None,
                        materialized_value_json: None,
                        diagnostic: Some(Self::stored_diagnostic(field, &failure)),
                    },
                );
                execution
                    .diagnostics
                    .push(Self::report_diagnostic(&path, field, &failure));
            }
            let frontmatter_unset: BTreeSet<String> = previous_fields
                .iter()
                .filter(|field| {
                    file.computed_fields
                        .get(*field)
                        .is_some_and(|entry| file.materialized_field_matches(field, entry))
                })
                .cloned()
                .collect();
            let changed_fields =
                Self::changed_patch_fields(file, &entries, &BTreeMap::new(), &frontmatter_unset);
            if !changed_fields.is_empty() {
                execution.fields_updated += changed_fields.len();
                execution.derived_field_patches.push(DerivedFieldPatch {
                    path,
                    expected_content_hash: file.content_hash.clone(),
                    fields: entries,
                    frontmatter_set: BTreeMap::new(),
                    frontmatter_unset,
                });
            }
        }
    }

    fn finish_schema_patch(
        context: &ModuleContext<'_>,
        overlay: Option<&OverlaySchema>,
        execution: &mut ModuleExecution,
    ) {
        execution.schema_patch = Some(super::rebuild_schema_without_computed_materializations(
            context, overlay,
        ));
    }

    fn refresh_one_schema<'a>(
        schema: &mut Schema,
        files: impl IntoIterator<Item = &'a StoredFile>,
    ) {
        super::refresh_computed_schema_stats(schema, files);
    }
}

impl Module for LookupRollupModule {
    fn descriptor(&self) -> ModuleDescriptor {
        ModuleDescriptor {
            id: LOOKUP_ROLLUP_MODULE_ID.to_string(),
            name: "Lookup & Rollup".to_string(),
            version: 1,
            always_on: true,
            hooks: vec![
                "full_ingest".to_string(),
                "files_changed".to_string(),
                "schema_changed".to_string(),
                "manual_run".to_string(),
            ],
        }
    }

    fn run(
        &self,
        context: &ModuleContext<'_>,
        event: &ModuleEvent,
    ) -> crate::Result<ModuleExecution> {
        let pre_load_schema_fingerprint =
            Schema::overlay_source_fingerprint(context.project_root).ok();
        let (overlay, schema_fingerprint) =
            match Schema::load_overlay_with_fingerprint(context.project_root) {
                Ok((Some(overlay), fingerprint)) => (overlay, fingerprint),
                Ok((None, fingerprint)) => {
                    let mut execution = ModuleExecution {
                        expected_schema_fingerprint: Some(fingerprint),
                        ..ModuleExecution::default()
                    };
                    Self::clear_owned_state(
                    context,
                    event,
                    "schema_overlay_missing",
                    "the schema overlay was removed; the cached Lookup/Rollup value was cleared",
                    &mut execution,
                );
                    Self::finish_schema_patch(context, None, &mut execution);
                    return Ok(execution);
                }
                Err(error) => {
                    let mut execution = ModuleExecution {
                        expected_schema_fingerprint: pre_load_schema_fingerprint,
                        ..ModuleExecution::default()
                    };
                    Self::clear_owned_state(
                        context,
                        event,
                        "invalid_schema",
                        &error.to_string(),
                        &mut execution,
                    );
                    if execution.diagnostics.is_empty() {
                        execution.diagnostics.push(ModuleDiagnostic {
                            path: None,
                            module: LOOKUP_ROLLUP_MODULE_ID.to_string(),
                            field: String::new(),
                            code: "invalid_schema".to_string(),
                            message: error.to_string(),
                            span_start: None,
                            span_end: None,
                        });
                    }
                    Self::finish_schema_patch(context, None, &mut execution);
                    return Ok(execution);
                }
            };
        Ok(self.build_execution(context, event, &overlay, &schema_fingerprint))
    }

    fn refresh_schema(
        &self,
        schema: &mut Schema,
        files: &HashMap<String, StoredFile>,
        scope: Option<&str>,
    ) {
        Self::refresh_one_schema(
            schema,
            files.iter().filter_map(|(path, file)| {
                scope
                    .is_none_or(|scope| crate::path_util::path_is_in_scope(path, scope))
                    .then_some(file)
            }),
        );
    }
}

struct RelationEvaluator<'a> {
    files: &'a HashMap<String, StoredFile>,
    overlay: &'a OverlaySchema,
    definitions: &'a BTreeMap<NodeKey, Definition>,
    engine: &'a FormulaEngine,
    known_files: HashSet<String>,
    outcomes: BTreeMap<NodeKey, EvaluationOutcome>,
    visiting: BTreeSet<NodeKey>,
    stack: Vec<NodeKey>,
    cycle_nodes: BTreeSet<NodeKey>,
    missing_dependency_states: BTreeMap<String, bool>,
    dependency_missing_paths: BTreeMap<NodeKey, BTreeSet<String>>,
    dependency_tokens: BTreeMap<NodeKey, Vec<Vec<u8>>>,
    dependency_paths: BTreeMap<NodeKey, BTreeSet<String>>,
}

impl<'a> RelationEvaluator<'a> {
    fn new(
        files: &'a HashMap<String, StoredFile>,
        overlay: &'a OverlaySchema,
        definitions: &'a BTreeMap<NodeKey, Definition>,
        engine: &'a FormulaEngine,
    ) -> Self {
        Self {
            files,
            overlay,
            definitions,
            engine,
            known_files: files.keys().cloned().collect(),
            outcomes: BTreeMap::new(),
            visiting: BTreeSet::new(),
            stack: Vec::new(),
            cycle_nodes: BTreeSet::new(),
            missing_dependency_states: BTreeMap::new(),
            dependency_missing_paths: BTreeMap::new(),
            dependency_tokens: BTreeMap::new(),
            dependency_paths: BTreeMap::new(),
        }
    }

    fn record_token(&mut self, token: impl Into<Vec<u8>>) {
        let token = token.into();
        for owner in &self.stack {
            self.dependency_tokens
                .entry(owner.clone())
                .or_default()
                .push(token.clone());
        }
    }

    fn record_path(&mut self, path: &str) {
        for owner in &self.stack {
            self.dependency_paths
                .entry(owner.clone())
                .or_default()
                .insert(path.to_string());
        }
    }

    fn record_missing_path(&mut self, path: &str) {
        self.missing_dependency_states
            .insert(path.to_string(), false);
        for owner in &self.stack {
            self.dependency_missing_paths
                .entry(owner.clone())
                .or_default()
                .insert(path.to_string());
        }
    }

    fn record_field(&mut self, path: &str, field: &str) {
        self.record_path(path);
        let mut token = Vec::new();
        token.extend_from_slice(b"field\0");
        token.extend_from_slice(path.as_bytes());
        token.push(0);
        token.extend_from_slice(field.as_bytes());
        token.push(0);
        match self.files.get(path) {
            None => token.extend_from_slice(b"<missing-document>"),
            Some(file) => {
                if self.definitions.contains_key(&NodeKey::new(path, field)) {
                    // The fresh recursively evaluated outcome is recorded by
                    // `optional_field_value`; never fingerprint the previous
                    // Lookup/Rollup materialization here.
                    token.extend_from_slice(b"<computed-field>");
                } else {
                    let value = file
                        .effective_frontmatter()
                        .and_then(|value| value.as_object().cloned())
                        .and_then(|frontmatter| frontmatter.get(field).cloned());
                    match value.and_then(|value| serde_json::to_vec(&value).ok()) {
                        Some(value) => token.extend_from_slice(&value),
                        None => token.extend_from_slice(b"<missing-field>"),
                    }
                }
                if let Some(entry) = file
                    .computed_fields
                    .get(field)
                    .filter(|entry| entry.module != LOOKUP_ROLLUP_MODULE_ID)
                {
                    token.push(0);
                    token.extend_from_slice(entry.module.as_bytes());
                    token.push(0);
                    token.extend_from_slice(entry.definition_fingerprint.as_bytes());
                    token.push(0);
                    if let Some(input) = &entry.input_fingerprint {
                        token.extend_from_slice(input.as_bytes());
                    }
                    if let Some(diagnostic) = &entry.diagnostic {
                        token.push(0);
                        token.extend_from_slice(diagnostic.code.as_bytes());
                        token.push(0);
                        token.extend_from_slice(diagnostic.message.as_bytes());
                    }
                }
            }
        }
        self.record_token(token);
    }

    fn record_outcome(
        &mut self,
        path: &str,
        field: &str,
        definition_fingerprint: &str,
        outcome: &EvaluationOutcome,
    ) {
        let mut token = Vec::new();
        token.extend_from_slice(b"computed\0");
        token.extend_from_slice(path.as_bytes());
        token.push(0);
        token.extend_from_slice(field.as_bytes());
        token.push(0);
        token.extend_from_slice(definition_fingerprint.as_bytes());
        token.push(0);
        match outcome {
            EvaluationOutcome::Value(value) => {
                token.extend_from_slice(b"value\0");
                if let Ok(value) = serde_json::to_vec(value) {
                    token.extend_from_slice(&value);
                }
            }
            EvaluationOutcome::Error(failure) => {
                token.extend_from_slice(b"error\0");
                token.extend_from_slice(failure.code.as_bytes());
                token.push(0);
                token.extend_from_slice(failure.message.as_bytes());
            }
        }
        self.record_token(token);
    }

    fn input_fingerprint(&self, key: &NodeKey, definition_fingerprint: &str) -> String {
        let mut hasher = Sha256::new();
        hasher.update(b"lookup_rollup_inputs_v3");
        hasher.update([0]);
        hasher.update(definition_fingerprint.as_bytes());
        if let Some(tokens) = self.dependency_tokens.get(key) {
            for token in tokens {
                hasher.update([0]);
                hasher.update(token);
            }
        }
        format!("{:x}", hasher.finalize())
    }

    fn dependency_paths(&self, key: &NodeKey) -> impl Iterator<Item = &String> {
        self.dependency_paths.get(key).into_iter().flatten()
    }

    fn dependency_path_state(&self, owner: &str, path: &str) -> ComputedDependencyPathState {
        if path == owner {
            return ComputedDependencyPathState {
                exists: true,
                content_hash: None,
            };
        }
        self.files.get(path).map_or(
            ComputedDependencyPathState {
                exists: false,
                content_hash: None,
            },
            |file| ComputedDependencyPathState {
                exists: true,
                content_hash: Some(file.content_hash.clone()),
            },
        )
    }

    fn dependency_snapshot(
        &self,
        key: &NodeKey,
        incoming_scope: Option<&str>,
    ) -> ComputedDependencySnapshot {
        let mut paths = BTreeMap::new();
        for path in self.dependency_paths(key) {
            paths.insert(path.clone(), self.dependency_path_state(&key.path, path));
        }
        for path in self.dependency_missing_paths.get(key).into_iter().flatten() {
            paths.insert(
                path.clone(),
                ComputedDependencyPathState {
                    exists: false,
                    content_hash: None,
                },
            );
        }

        let incoming_scopes = incoming_scope
            .map(|scope| {
                let members = self
                    .files
                    .iter()
                    .filter(|(path, _)| crate::path_util::path_is_in_scope(path, scope))
                    .map(|(path, file)| {
                        (
                            path.clone(),
                            ComputedDependencyPathState {
                                exists: true,
                                content_hash: (path != &key.path)
                                    .then(|| file.content_hash.clone()),
                            },
                        )
                    })
                    .collect();
                BTreeMap::from([(scope.to_string(), members)])
            })
            .unwrap_or_default();

        ComputedDependencySnapshot::from_states(paths, incoming_scopes)
    }

    fn cached_outcome(
        &self,
        key: &NodeKey,
        definition: &Definition,
        input_fingerprint: &str,
    ) -> Option<EvaluationOutcome> {
        let file = self.files.get(&key.path)?;
        let entry = file.computed_fields.get(&key.field)?;
        if entry.module != LOOKUP_ROLLUP_MODULE_ID
            || entry.definition_fingerprint != definition.fingerprint
            || entry.input_fingerprint.as_deref() != Some(input_fingerprint)
            || !LookupRollupModule::materialization_matches(file, &key.field, entry)
        {
            return None;
        }
        if let Some(diagnostic) = &entry.diagnostic {
            return Some(EvaluationOutcome::Error(EvaluationFailure {
                code: diagnostic.code.clone(),
                message: diagnostic.message.clone(),
                span_start: diagnostic.span_start,
                span_end: diagnostic.span_end,
            }));
        }
        entry
            .value_json
            .as_deref()
            .and_then(|value| serde_json::from_str(value).ok())
            .map(EvaluationOutcome::Value)
    }

    fn evaluate_node(&mut self, key: &NodeKey) -> EvaluationOutcome {
        if let Some(outcome) = self.outcomes.get(key) {
            return outcome.clone();
        }
        if self.visiting.contains(key) {
            let start = self
                .stack
                .iter()
                .position(|candidate| candidate == key)
                .unwrap_or(0);
            self.cycle_nodes.extend(self.stack[start..].iter().cloned());
            self.cycle_nodes.insert(key.clone());
            return EvaluationOutcome::Error(EvaluationFailure::new(
                "dependency_cycle",
                format!(
                    "Lookup/Rollup field `{}.{}` participates in a dependency cycle",
                    key.path, key.field
                ),
            ));
        }
        if self.stack.len() >= MAX_DEPENDENCY_DEPTH {
            return EvaluationOutcome::Error(EvaluationFailure::new(
                "dependency_limit",
                format!(
                    "Lookup/Rollup dependency depth exceeded the limit of {MAX_DEPENDENCY_DEPTH} nodes while evaluating `{}.{}`",
                    key.path, key.field
                ),
            ));
        }
        let Some(definition) = self.definitions.get(key).cloned() else {
            return EvaluationOutcome::Error(EvaluationFailure::new(
                "definition_missing",
                format!(
                    "computed definition `{}.{}` is missing",
                    key.path, key.field
                ),
            ));
        };

        self.visiting.insert(key.clone());
        self.stack.push(key.clone());
        let prepared = self.prepare_definition(key, &definition);
        let input_fingerprint = self.input_fingerprint(key, &definition.fingerprint);
        let mut outcome = match prepared {
            Ok(prepared) => self
                .cached_outcome(key, &definition, &input_fingerprint)
                .unwrap_or_else(|| match prepared {
                    PreparedDefinition::Lookup(value) => EvaluationOutcome::Value(value),
                    PreparedDefinition::Rollup(values) => {
                        match self.evaluate_rollup(key, &definition, values) {
                            Ok(value) => EvaluationOutcome::Value(value),
                            Err(failure) => EvaluationOutcome::Error(failure),
                        }
                    }
                }),
            Err(failure) => EvaluationOutcome::Error(failure),
        };
        self.stack.pop();
        self.visiting.remove(key);
        if self.cycle_nodes.contains(key) {
            outcome = EvaluationOutcome::Error(EvaluationFailure::new(
                "dependency_cycle",
                format!(
                    "Lookup/Rollup field `{}.{}` participates in a dependency cycle",
                    key.path, key.field
                ),
            ));
        }
        self.outcomes.insert(key.clone(), outcome.clone());
        outcome
    }

    fn prepare_definition(
        &mut self,
        key: &NodeKey,
        definition: &Definition,
    ) -> Result<PreparedDefinition, EvaluationFailure> {
        let selection = match definition.direction {
            RelationDirection::Outgoing => {
                self.outgoing_selection(&key.path, &definition.relation_field)?
            }
            RelationDirection::Incoming => self.incoming_selection(
                &key.path,
                &definition.relation_field,
                definition.relation_scope.as_deref().unwrap_or_default(),
            )?,
        };

        match definition.kind {
            DefinitionKind::Lookup => match selection {
                RelationSelection::Missing => Ok(PreparedDefinition::Lookup(JsonValue::Null)),
                RelationSelection::Scalar(path) => self
                    .target_value(&path, &definition.target_field)
                    .map(PreparedDefinition::Lookup),
                RelationSelection::List(paths) => paths
                    .into_iter()
                    .map(|path| self.target_value(&path, &definition.target_field))
                    .collect::<Result<Vec<_>, _>>()
                    .map(JsonValue::Array)
                    .map(PreparedDefinition::Lookup),
            },
            DefinitionKind::Rollup => {
                let paths = match selection {
                    RelationSelection::Missing => Vec::new(),
                    RelationSelection::Scalar(path) => vec![path],
                    RelationSelection::List(paths) => paths,
                };
                let values = paths
                    .into_iter()
                    .map(|path| self.target_value(&path, &definition.target_field))
                    .collect::<Result<Vec<_>, _>>()?;
                Ok(PreparedDefinition::Rollup(values))
            }
        }
    }

    fn evaluate_rollup(
        &self,
        _key: &NodeKey,
        definition: &Definition,
        values: Vec<JsonValue>,
    ) -> Result<JsonValue, EvaluationFailure> {
        let formula = definition
            .formula
            .as_deref()
            .ok_or_else(|| EvaluationFailure::new("invalid_schema", "rollup formula is missing"))?;
        let result_type = definition.result_type.ok_or_else(|| {
            EvaluationFailure::new("invalid_schema", "rollup result_type is missing")
        })?;
        // The authored output key may itself be `values`. Compile under a
        // private result name so Formula's same-name materialization guard does
        // not hide Rollup's reserved input array or create a false self-cycle.
        const RESULT_FIELD: &str = "__lookup_rollup_result__";
        let program =
            self.engine
                .compile([FormulaDefinition::new(RESULT_FIELD, formula, result_type)]);
        let inputs = JsonMap::from_iter([("values".to_string(), JsonValue::Array(values))]);
        let evaluation = program.evaluate(&inputs);
        if let Some(diagnostic) = evaluation.errors.get(RESULT_FIELD) {
            return Err(EvaluationFailure {
                code: diagnostic.code.clone(),
                message: diagnostic.message.clone(),
                span_start: diagnostic.span.as_ref().map(|span| span.start as usize),
                span_end: diagnostic.span.as_ref().map(|span| span.end as usize),
            });
        }
        evaluation
            .values
            .get(RESULT_FIELD)
            .ok_or_else(|| {
                EvaluationFailure::new("undefined_result", "rollup formula did not produce a value")
            })?
            .to_json()
            .map_err(|diagnostic| EvaluationFailure {
                code: diagnostic.code,
                message: diagnostic.message,
                span_start: diagnostic.span.as_ref().map(|span| span.start as usize),
                span_end: diagnostic.span.as_ref().map(|span| span.end as usize),
            })
    }

    fn outgoing_selection(
        &mut self,
        source: &str,
        relation_field: &str,
    ) -> Result<RelationSelection, EvaluationFailure> {
        self.record_field(source, relation_field);
        let resolved_overlay = Schema::resolve_overlay_for_path(self.overlay, Some(source));
        let relation_overlay = resolved_overlay.get(relation_field);
        if let Some(field) = relation_overlay {
            let declared_type = field
                .field_type
                .as_deref()
                .and_then(crate::schema::parse_field_type_str);
            let explicitly_relation = matches!(declared_type, Some(FieldType::Relation))
                || (field.field_type.is_none() && field.target.is_some());
            if field.field_type.is_some() && !explicitly_relation {
                return Err(EvaluationFailure::new(
                    "relation_field_type",
                    format!(
                        "selected relation field `{source}.{relation_field}` must have field_type `relation`, found `{}`",
                        field.field_type.as_deref().unwrap_or_default()
                    ),
                ));
            }
        }

        let relation_value = self.optional_field_value(source, relation_field)?;
        let Some(value) = relation_value else {
            return Ok(RelationSelection::Missing);
        };
        if value.is_null() {
            return Ok(RelationSelection::Missing);
        }
        // With no explicit type override, preserve the existing inferred-
        // Relation workflow. Empty lists are a valid zero-target relation and
        // cannot be classified from their contents alone.
        if relation_overlay.is_none_or(|field| field.field_type.is_none() && field.target.is_none())
            && !matches!(&value, JsonValue::Array(values) if values.is_empty())
            && crate::schema::infer_field_type(&value) != FieldType::Relation
        {
            return Err(EvaluationFailure::new(
                "relation_field_type",
                format!(
                    "selected relation field `{source}.{relation_field}` does not contain Relation values"
                ),
            ));
        }
        let target_folder = relation_overlay.and_then(|field| field.target.clone());
        match value {
            JsonValue::String(raw) => self
                .resolve_relation(source, relation_field, &raw, target_folder.as_deref())
                .map(RelationSelection::Scalar),
            JsonValue::Array(values) => values
                .iter()
                .map(|value| {
                    let raw = value.as_str().ok_or_else(|| {
                        EvaluationFailure::new(
                            "invalid_relation",
                            format!(
                                "relation field `{source}.{relation_field}` contains a non-string value"
                            ),
                        )
                    })?;
                    self.resolve_relation(source, relation_field, raw, target_folder.as_deref())
                })
                .collect::<Result<Vec<_>, _>>()
                .map(RelationSelection::List),
            _ => Err(EvaluationFailure::new(
                "invalid_relation",
                format!(
                    "relation field `{source}.{relation_field}` must be a link string, list, or null"
                ),
            )),
        }
    }

    fn incoming_selection(
        &mut self,
        owner: &str,
        relation_field: &str,
        relation_scope: &str,
    ) -> Result<RelationSelection, EvaluationFailure> {
        let mut candidates: Vec<String> = self
            .files
            .keys()
            .filter(|path| crate::path_util::path_is_in_scope(path, relation_scope))
            .cloned()
            .collect();
        candidates.sort();
        let mut membership = Vec::from(b"incoming-membership\0".as_slice());
        membership.extend_from_slice(relation_scope.as_bytes());
        for candidate in &candidates {
            membership.push(0);
            membership.extend_from_slice(candidate.as_bytes());
            self.record_path(candidate);
        }
        self.record_token(membership);
        let mut matched = BTreeSet::new();
        for source in candidates {
            let selection = self.outgoing_selection(&source, relation_field)?;
            let points_to_owner = match selection {
                RelationSelection::Missing => false,
                RelationSelection::Scalar(path) => path == owner,
                RelationSelection::List(paths) => paths.iter().any(|path| path == owner),
            };
            if points_to_owner {
                matched.insert(source);
            }
        }
        Ok(RelationSelection::List(matched.into_iter().collect()))
    }

    fn resolve_relation(
        &mut self,
        source: &str,
        relation_field: &str,
        raw: &str,
        target_folder: Option<&str>,
    ) -> Result<String, EvaluationFailure> {
        let mut relation_token = Vec::from(b"relation\0".as_slice());
        relation_token.extend_from_slice(source.as_bytes());
        relation_token.push(0);
        relation_token.extend_from_slice(relation_field.as_bytes());
        relation_token.push(0);
        relation_token.extend_from_slice(raw.as_bytes());
        relation_token.push(0);
        relation_token.extend_from_slice(target_folder.unwrap_or_default().as_bytes());
        self.record_token(relation_token);
        let parsed = crate::relations::parse_link_shaped(raw).ok_or_else(|| {
            EvaluationFailure::new(
                "invalid_relation",
                format!("`{raw}` in `{source}.{relation_field}` is not a relation link"),
            )
        })?;
        if crate::relations::parsed_link_kind(&parsed)
            != crate::relations::FrontmatterLinkKind::Relation
        {
            return Err(EvaluationFailure::new(
                "invalid_relation",
                format!("`{raw}` in `{source}.{relation_field}` points to a non-Markdown file"),
            ));
        }
        let candidates =
            crate::relations::relation_target_candidates(source, &parsed.target, target_folder);
        let (path, exists) = crate::relations::resolve_relation_target(
            source,
            &parsed.target,
            target_folder,
            &self.known_files,
        )
        .ok_or_else(|| {
            EvaluationFailure::new(
                "invalid_relation",
                format!("`{raw}` in `{source}.{relation_field}` has no resolvable target"),
            )
        })?;

        // Winner selection is ordered. Snapshot every absent candidate before
        // the selected existing path, plus the selected path itself. If no
        // candidate exists, every candidate can change the outcome. The
        // expected absence deliberately comes from the coherent indexed
        // snapshot rather than a later filesystem probe; a file that appears
        // during evaluation must fail dependency verification before writeback.
        let considered = if exists {
            candidates
                .iter()
                .position(|candidate| candidate == &path)
                .map_or(candidates.len(), |position| position + 1)
        } else {
            candidates.len()
        };
        for candidate in candidates.iter().take(considered) {
            if self.known_files.contains(candidate) {
                self.record_path(candidate);
            } else {
                self.record_missing_path(candidate);
            }
        }

        if !exists {
            let mut resolution = Vec::from(b"resolved\0missing\0".as_slice());
            resolution.extend_from_slice(path.as_bytes());
            self.record_token(resolution);
            return Err(EvaluationFailure::new(
                "relation_unresolved",
                format!("relation `{source}.{relation_field}` points to missing `{path}`"),
            ));
        }
        let mut resolution = Vec::from(b"resolved\0existing\0".as_slice());
        resolution.extend_from_slice(path.as_bytes());
        self.record_token(resolution);
        Ok(path)
    }

    fn optional_field_value(
        &mut self,
        path: &str,
        field: &str,
    ) -> Result<Option<JsonValue>, EvaluationFailure> {
        self.record_field(path, field);
        let key = NodeKey::new(path, field);
        if self.definitions.contains_key(&key) {
            let definition_fingerprint = self.definitions[&key].fingerprint.clone();
            let outcome = self.evaluate_node(&key);
            self.record_outcome(path, field, &definition_fingerprint, &outcome);
            return match outcome {
                EvaluationOutcome::Value(value) => Ok(Some(value)),
                EvaluationOutcome::Error(failure) => {
                    Err(EvaluationFailure::dependency(path, field, &failure))
                }
            };
        }
        let formula_defined = Schema::resolve_overlay_for_path(self.overlay, Some(path))
            .get(field)
            .and_then(|definition| definition.field_type.as_deref())
            .and_then(crate::schema::parse_field_type_str)
            .is_some_and(|field_type| field_type == FieldType::Formula);
        if let Some(diagnostic) = self
            .files
            .get(path)
            .and_then(|file| file.computed_fields.get(field))
            .filter(|entry| formula_defined && entry.module == crate::formula::FORMULA_MODULE_ID)
            .and_then(|entry| entry.diagnostic.as_ref())
        {
            let cause = EvaluationFailure {
                code: diagnostic.code.clone(),
                message: diagnostic.message.clone(),
                span_start: diagnostic.span_start,
                span_end: diagnostic.span_end,
            };
            return Err(EvaluationFailure::dependency(path, field, &cause));
        }
        Ok(self
            .raw_frontmatter(path)
            .and_then(|map| map.get(field).cloned()))
    }

    fn target_value(&mut self, path: &str, field: &str) -> Result<JsonValue, EvaluationFailure> {
        self.optional_field_value(path, field)?.ok_or_else(|| {
            EvaluationFailure::new(
                "target_field_missing",
                format!("related document `{path}` has no target field `{field}`"),
            )
        })
    }

    fn raw_frontmatter(&self, path: &str) -> Option<JsonMap<String, JsonValue>> {
        let file = self.files.get(path)?;
        let mut value = file.effective_frontmatter()?;
        let object = value.as_object_mut()?;
        // Never read this module's stale materialization. Current definitions
        // are supplied through recursive evaluation above.
        for (field, entry) in &file.computed_fields {
            if entry.module == LOOKUP_ROLLUP_MODULE_ID
                && file.materialized_field_matches(field, entry)
            {
                object.remove(field);
            }
        }
        Some(object.clone())
    }
}

#[derive(Debug, Default)]
pub(super) struct ManualDependencyPlan {
    pub formula_paths: Vec<String>,
    pub lookup_rollup_paths: Vec<String>,
}

/// Resolve the exact manual output owners and the cross-scope prerequisite /
/// downstream closure. This topology pass reads raw Relation values only; it
/// deliberately does not trust the link graph, which discards order and
/// duplicates and can lag an overlay edit.
pub(super) fn plan_manual_dependencies(
    files: &HashMap<String, StoredFile>,
    overlay: &OverlaySchema,
    requested_module: &str,
    scope: Option<&str>,
) -> ManualDependencyPlan {
    let prefix = super::normalize_module_scope(scope);
    let requested_paths: BTreeSet<String> = files
        .keys()
        .filter(|path| {
            prefix
                .as_deref()
                .is_none_or(|scope| crate::path_util::path_is_in_scope(path, scope))
        })
        .cloned()
        .collect();
    let definitions = LookupRollupModule::collect_definitions(overlay, files);
    let known_files: HashSet<String> = files.keys().cloned().collect();

    let raw_targets = |source: &str, relation_field: &str| -> Vec<String> {
        let resolved_overlay = Schema::resolve_overlay_for_path(overlay, Some(source));
        let relation_overlay = resolved_overlay.get(relation_field);
        if relation_overlay.is_some_and(|field| {
            field
                .field_type
                .as_deref()
                .and_then(crate::schema::parse_field_type_str)
                .is_some_and(|field_type| field_type != FieldType::Relation)
        }) {
            return Vec::new();
        }
        let target_folder = relation_overlay.and_then(|field| field.target.as_deref());
        let Some(value) = files
            .get(source)
            .and_then(|file| file.frontmatter.as_deref())
            .and_then(|raw| serde_json::from_str::<JsonValue>(raw).ok())
            .and_then(|value| value.as_object().cloned())
            .and_then(|frontmatter| frontmatter.get(relation_field).cloned())
        else {
            return Vec::new();
        };
        let values: Vec<&str> = match &value {
            JsonValue::String(value) => vec![value],
            JsonValue::Array(values) => values.iter().filter_map(JsonValue::as_str).collect(),
            _ => Vec::new(),
        };
        values
            .into_iter()
            .filter_map(crate::relations::parse_link_shaped)
            .filter(|parsed| {
                crate::relations::parsed_link_kind(parsed)
                    == crate::relations::FrontmatterLinkKind::Relation
            })
            .filter_map(|parsed| {
                crate::relations::resolve_relation_target(
                    source,
                    &parsed.target,
                    target_folder,
                    &known_files,
                )
            })
            .filter_map(|(path, exists)| exists.then_some(path))
            .collect()
    };

    let mut computed_edges = BTreeMap::<NodeKey, BTreeSet<NodeKey>>::new();
    let mut formula_edges = BTreeMap::<NodeKey, BTreeSet<String>>::new();
    for (key, definition) in &definitions {
        let targets = match definition.direction {
            RelationDirection::Outgoing => raw_targets(&key.path, &definition.relation_field),
            RelationDirection::Incoming => {
                let scope = definition.relation_scope.as_deref().unwrap_or_default();
                let mut sources: Vec<String> = files
                    .keys()
                    .filter(|path| crate::path_util::path_is_in_scope(path, scope))
                    .filter(|path| {
                        raw_targets(path, &definition.relation_field)
                            .iter()
                            .any(|target| target == &key.path)
                    })
                    .cloned()
                    .collect();
                sources.sort();
                sources.dedup();
                sources
            }
        };
        for target_path in targets {
            let target = NodeKey::new(&target_path, &definition.target_field);
            if definitions.contains_key(&target) {
                computed_edges
                    .entry(key.clone())
                    .or_default()
                    .insert(target);
                continue;
            }
            let target_overlay = Schema::resolve_overlay_for_path(overlay, Some(&target_path));
            if target_overlay
                .get(&definition.target_field)
                .and_then(|field| field.field_type.as_deref())
                .and_then(crate::schema::parse_field_type_str)
                == Some(FieldType::Formula)
            {
                formula_edges
                    .entry(key.clone())
                    .or_default()
                    .insert(target_path);
            }
        }
    }

    if requested_module == crate::formula::FORMULA_MODULE_ID {
        let mut affected = BTreeSet::<NodeKey>::new();
        loop {
            let before = affected.len();
            for key in definitions.keys() {
                let directly_affected = formula_edges
                    .get(key)
                    .is_some_and(|paths| !paths.is_disjoint(&requested_paths));
                let transitively_affected = computed_edges
                    .get(key)
                    .is_some_and(|dependencies| !dependencies.is_disjoint(&affected));
                if directly_affected || transitively_affected {
                    affected.insert(key.clone());
                }
            }
            if affected.len() == before {
                break;
            }
        }
        return ManualDependencyPlan {
            formula_paths: requested_paths.into_iter().collect(),
            lookup_rollup_paths: affected.into_iter().map(|key| key.path).collect(),
        };
    }

    let mut formula_paths = BTreeSet::new();
    let mut visited = BTreeSet::new();
    let mut pending: Vec<NodeKey> = definitions
        .keys()
        .filter(|key| requested_paths.contains(&key.path))
        .cloned()
        .collect();
    while let Some(key) = pending.pop() {
        if !visited.insert(key.clone()) {
            continue;
        }
        if let Some(paths) = formula_edges.get(&key) {
            formula_paths.extend(paths.iter().cloned());
        }
        if let Some(dependencies) = computed_edges.get(&key) {
            pending.extend(dependencies.iter().cloned());
        }
    }

    ManualDependencyPlan {
        formula_paths: formula_paths.into_iter().collect(),
        lookup_rollup_paths: requested_paths.into_iter().collect(),
    }
}

fn definition_fingerprint(
    kind: DefinitionKind,
    relation_field: &str,
    target_field: &str,
    direction: RelationDirection,
    relation_scope: Option<&str>,
    formula: Option<&str>,
    result_type: Option<FormulaResultType>,
) -> String {
    let mut hasher = Sha256::new();
    hasher.update(match kind {
        DefinitionKind::Lookup => b"lookup".as_slice(),
        DefinitionKind::Rollup => b"rollup".as_slice(),
    });
    for value in [
        relation_field,
        target_field,
        match direction {
            RelationDirection::Outgoing => "outgoing",
            RelationDirection::Incoming => "incoming",
        },
        relation_scope.unwrap_or_default(),
        formula.unwrap_or_default(),
    ] {
        hasher.update([0]);
        hasher.update(value.as_bytes());
    }
    if let Some(result_type) = result_type {
        hasher.update([0]);
        hasher.update(result_type.to_string().as_bytes());
    }
    hasher
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::index::state::Index;
    use crate::index::types::EmbeddingConfig;
    use crate::modules::ModuleRunner;
    use std::path::Path;
    use tempfile::TempDir;

    fn index(root: &Path) -> Index {
        Index::create(
            &root.join("index"),
            &EmbeddingConfig {
                provider: "test".to_string(),
                model: "test".to_string(),
                dimensions: 2,
            },
        )
        .unwrap()
    }

    fn add_file(root: &Path, index: &Index, path: &str, source: &str) {
        let absolute = root.join(path);
        std::fs::create_dir_all(absolute.parent().unwrap()).unwrap();
        std::fs::write(&absolute, source).unwrap();
        let parsed = crate::parser::parse_markdown_file(root, Path::new(path)).unwrap();
        index.upsert(&parsed, &[], &[]).unwrap();
        index.save().unwrap();
    }

    fn parsed_frontmatter(root: &Path, path: &str) -> JsonValue {
        crate::parser::parse_markdown_file(root, Path::new(path))
            .unwrap()
            .frontmatter
            .unwrap()
    }

    fn parsed_frontmatter_or_empty(root: &Path, path: &str) -> JsonValue {
        crate::parser::parse_markdown_file(root, Path::new(path))
            .unwrap()
            .frontmatter
            .unwrap_or_else(|| serde_json::json!({}))
    }

    fn seed_materialized(index: &Index, path: &str, field: &str, value: JsonValue) {
        let value_json = serde_json::to_string(&value).unwrap();
        index
            .replace_computed_fields(
                path,
                HashMap::from([(
                    field.to_string(),
                    ComputedFieldEntry {
                        module: LOOKUP_ROLLUP_MODULE_ID.to_string(),
                        definition_fingerprint: "previous".to_string(),
                        input_fingerprint: None,
                        dependency_snapshot: ComputedDependencySnapshot::default(),
                        value_json: Some(value_json.clone()),
                        materialized_value_json: Some(value_json),
                        diagnostic: None,
                    },
                )]),
            )
            .unwrap();
        index.save().unwrap();
    }

    struct HigherPriorityTargetAppears;

    impl Module for HigherPriorityTargetAppears {
        fn descriptor(&self) -> ModuleDescriptor {
            LookupRollupModule::default().descriptor()
        }

        fn run(
            &self,
            context: &ModuleContext<'_>,
            event: &ModuleEvent,
        ) -> crate::Result<ModuleExecution> {
            let execution = LookupRollupModule::default().run(context, event)?;
            let candidate = context.project_root.join("clients/acme.md");
            std::fs::create_dir_all(candidate.parent().unwrap())?;
            std::fs::write(
                candidate,
                "---\ndomain: higher-priority.example\n---\nHigher priority\n",
            )?;
            Ok(execution)
        }
    }

    #[test]
    fn higher_priority_target_appearing_after_resolution_rejects_owner_write() {
        let dir = TempDir::new().unwrap();
        let root = dir.path();
        std::fs::write(
            root.join(".markdownvdb.schema.yml"),
            r#"scopes:
  contacts:
    fields:
      client:
        field_type: relation
      client_domain:
        field_type: lookup
        relation_field: client
        target_field: domain
"#,
        )
        .unwrap();
        let index = index(root);
        add_file(
            root,
            &index,
            "contacts/clients/acme.md",
            "---\ndomain: fallback.example\n---\nFallback\n",
        );
        add_file(
            root,
            &index,
            "contacts/alice.md",
            "---\nclient: '[[clients/acme]]'\n---\nAlice\n",
        );

        let report = ModuleRunner::new(vec![Box::new(HigherPriorityTargetAppears)])
            .run_one(
                LOOKUP_ROLLUP_MODULE_ID,
                root,
                &index,
                &ModuleEvent::ManualPaths {
                    paths: vec!["contacts/alice.md".to_string()],
                },
            )
            .unwrap();

        assert_eq!(report.fields_updated, 0);
        assert!(report.diagnostics.iter().any(|diagnostic| {
            diagnostic.path.as_deref() == Some("contacts/alice.md")
                && diagnostic.field == "client_domain"
                && diagnostic.code == "dependency_changed"
        }));
        assert!(parsed_frontmatter(root, "contacts/alice.md")
            .get("client_domain")
            .is_none());
        assert_eq!(
            index.get_computed_fields("contacts/alice.md").unwrap()["client_domain"]
                .diagnostic
                .as_ref()
                .unwrap()
                .code,
            "dependency_changed"
        );
    }

    #[test]
    fn outgoing_lookup_preserves_scalar_list_order_duplicates_and_nesting() {
        let dir = TempDir::new().unwrap();
        let root = dir.path();
        std::fs::write(
            root.join(".markdownvdb.schema.yml"),
            r#"scopes:
  contacts:
    fields:
      domain:
        field_type: lookup
        relation_field: clients
        target_field: domain
      labels:
        field_type: lookup
        relation_field: clients
        target_field: labels
"#,
        )
        .unwrap();
        let index = index(root);
        add_file(
            root,
            &index,
            "clients/a.md",
            "---\ndomain: a.example\nlabels: [one, two]\n---\nA\n",
        );
        add_file(
            root,
            &index,
            "clients/b.md",
            "---\ndomain: b.example\nlabels: [three]\n---\nB\n",
        );
        add_file(
            root,
            &index,
            "clients/nulls.md",
            "---\ndomain: null\nlabels: null\n---\nNulls\n",
        );
        add_file(
            root,
            &index,
            "contacts/scalar.md",
            "---\nclients: '[[clients/a]]'\n---\nScalar\n",
        );
        add_file(
            root,
            &index,
            "contacts/list.md",
            "---\nclients: ['[[clients/b]]', '[[clients/a]]', '[[clients/b]]']\n---\nList\n",
        );
        add_file(
            root,
            &index,
            "contacts/missing.md",
            "---\nname: Missing\n---\n",
        );
        add_file(
            root,
            &index,
            "contacts/null.md",
            "---\nclients: null\n---\nNull\n",
        );
        add_file(
            root,
            &index,
            "contacts/empty.md",
            "---\nclients: []\n---\nEmpty\n",
        );
        add_file(
            root,
            &index,
            "contacts/null-target.md",
            "---\nclients: '[[clients/nulls]]'\n---\nNull target\n",
        );

        let reports = ModuleRunner::builtins()
            .run(root, &index, &ModuleEvent::FullIngest)
            .unwrap();
        assert_eq!(reports[1].module, LOOKUP_ROLLUP_MODULE_ID);
        assert!(
            reports[1].diagnostics.is_empty(),
            "{:?}",
            reports[1].diagnostics
        );

        let scalar = parsed_frontmatter(root, "contacts/scalar.md");
        assert_eq!(scalar["domain"], JsonValue::String("a.example".to_string()));
        assert_eq!(scalar["labels"], serde_json::json!(["one", "two"]));
        let scalar_state = index.get_computed_fields("contacts/scalar.md").unwrap();
        let scalar_entry = &scalar_state["domain"];
        assert!(scalar_entry.input_fingerprint.is_some());
        assert!(scalar_entry.dependency_snapshot.paths["contacts/scalar.md"].exists);
        assert!(scalar_entry.dependency_snapshot.paths["contacts/scalar.md"]
            .content_hash
            .is_none());
        let target = index.get_file("clients/a.md").unwrap();
        assert_eq!(
            scalar_entry.dependency_snapshot.paths["clients/a.md"]
                .content_hash
                .as_deref(),
            Some(target.content_hash.as_str())
        );
        let public_entry = serde_json::to_value(scalar_entry).unwrap();
        assert!(public_entry.get("input_fingerprint").is_none());
        assert!(public_entry.get("dependency_snapshot").is_none());
        let list = parsed_frontmatter(root, "contacts/list.md");
        assert_eq!(
            list["domain"],
            serde_json::json!(["b.example", "a.example", "b.example"])
        );
        assert_eq!(
            list["labels"],
            serde_json::json!([["three"], ["one", "two"], ["three"]])
        );
        for path in [
            "contacts/missing.md",
            "contacts/null.md",
            "contacts/null-target.md",
        ] {
            let contact = parsed_frontmatter(root, path);
            assert_eq!(contact["domain"], JsonValue::Null, "{path}");
            assert_eq!(contact["labels"], JsonValue::Null, "{path}");
        }
        let empty = parsed_frontmatter(root, "contacts/empty.md");
        assert_eq!(empty["domain"], serde_json::json!([]));
        assert_eq!(empty["labels"], serde_json::json!([]));
    }

    #[test]
    fn incoming_rollup_reads_fresh_formula_targets_and_deduplicates_sources() {
        let dir = TempDir::new().unwrap();
        let root = dir.path();
        std::fs::write(
            root.join(".markdownvdb.schema.yml"),
            r#"scopes:
  invoices:
    fields:
      total:
        field_type: formula
        formula: price * quantity
        result_type: number
  clients:
    fields:
      invoiced:
        field_type: rollup
        relation_field: client
        target_field: total
        relation_direction: incoming
        relation_scope: invoices
        formula: values.reduce((sum, value) => sum + value, 0)
        result_type: number
"#,
        )
        .unwrap();
        let index = index(root);
        add_file(
            root,
            &index,
            "clients/acme.md",
            "---\nname: Acme\n---\nAcme\n",
        );
        add_file(
            root,
            &index,
            "invoices/a.md",
            "---\nclient: ['[[clients/acme]]', '[[clients/acme]]']\nprice: 0.1\nquantity: 3\n---\nA\n",
        );
        add_file(
            root,
            &index,
            "invoices/b.md",
            "---\nclient: '[[clients/acme]]'\nprice: 2\nquantity: 4\n---\nB\n",
        );

        let reports = ModuleRunner::builtins()
            .run(root, &index, &ModuleEvent::FullIngest)
            .unwrap();
        assert_eq!(reports[0].module, crate::formula::FORMULA_MODULE_ID);
        assert_eq!(reports[1].module, LOOKUP_ROLLUP_MODULE_ID);
        assert!(
            reports[1].diagnostics.is_empty(),
            "{:?}",
            reports[1].diagnostics
        );
        let client = parsed_frontmatter(root, "clients/acme.md");
        assert_eq!(client["invoiced"], serde_json::json!(8.3));

        let formula_state = index.get_computed_fields("invoices/a.md").unwrap();
        let formula_entry = &formula_state["total"];
        assert!(formula_entry.input_fingerprint.is_some());
        assert!(formula_entry.dependency_snapshot.paths["invoices/a.md"].exists);
        assert!(formula_entry.dependency_snapshot.paths["invoices/a.md"]
            .content_hash
            .is_none());

        let rollup_state = index.get_computed_fields("clients/acme.md").unwrap();
        let rollup_entry = &rollup_state["invoiced"];
        let incoming = &rollup_entry.dependency_snapshot.incoming_scopes["invoices"];
        assert_eq!(
            incoming.keys().cloned().collect::<Vec<_>>(),
            vec!["invoices/a.md".to_string(), "invoices/b.md".to_string()]
        );
        for candidate in incoming.values() {
            assert!(candidate.exists);
            assert!(candidate.content_hash.is_some());
        }
    }

    #[test]
    fn rollup_presets_handle_values_and_empty_relations() {
        let dir = TempDir::new().unwrap();
        let root = dir.path();
        std::fs::write(
            root.join(".markdownvdb.schema.yml"),
            r#"scopes:
  clients:
    fields:
      invoices:
        field_type: relation
        target: invoices
      sum:
        field_type: rollup
        relation_field: invoices
        target_field: amount
        formula: values.reduce((sum, value) => sum + value, 0)
        result_type: number
      count:
        field_type: rollup
        relation_field: invoices
        target_field: amount
        formula: values.length
        result_type: number
      average:
        field_type: rollup
        relation_field: invoices
        target_field: amount
        formula: 'values.length ? values.reduce((sum, value) => sum + value, 0) / values.length : null'
        result_type: number
      minimum:
        field_type: rollup
        relation_field: invoices
        target_field: amount
        formula: 'values.length ? values.reduce((minimum, value) => Math.min(minimum, value)) : null'
        result_type: number
      maximum:
        field_type: rollup
        relation_field: invoices
        target_field: amount
        formula: 'values.length ? values.reduce((maximum, value) => Math.max(maximum, value)) : null'
        result_type: number
"#,
        )
        .unwrap();
        let index = index(root);
        add_file(root, &index, "invoices/a.md", "---\namount: 5\n---\nA\n");
        add_file(root, &index, "invoices/b.md", "---\namount: -2\n---\nB\n");
        add_file(
            root,
            &index,
            "clients/filled.md",
            "---\ninvoices: ['[[a]]', '[[b]]']\n---\nFilled\n",
        );
        add_file(
            root,
            &index,
            "clients/empty.md",
            "---\ninvoices: []\n---\nEmpty\n",
        );
        add_file(
            root,
            &index,
            "clients/null.md",
            "---\ninvoices: null\n---\nNull\n",
        );

        let reports = ModuleRunner::builtins()
            .run(root, &index, &ModuleEvent::FullIngest)
            .unwrap();
        assert!(
            reports[1].diagnostics.is_empty(),
            "{:?}",
            reports[1].diagnostics
        );

        let filled = parsed_frontmatter(root, "clients/filled.md");
        assert_eq!(filled["sum"], serde_json::json!(3));
        assert_eq!(filled["count"], serde_json::json!(2));
        assert_eq!(filled["average"], serde_json::json!(1.5));
        assert_eq!(filled["minimum"], serde_json::json!(-2));
        assert_eq!(filled["maximum"], serde_json::json!(5));

        for path in ["clients/empty.md", "clients/null.md"] {
            let empty = parsed_frontmatter(root, path);
            assert_eq!(empty["sum"], serde_json::json!(0), "{path}");
            assert_eq!(empty["count"], serde_json::json!(0), "{path}");
            assert_eq!(empty["average"], JsonValue::Null, "{path}");
            assert_eq!(empty["minimum"], JsonValue::Null, "{path}");
            assert_eq!(empty["maximum"], JsonValue::Null, "{path}");
        }
    }

    #[test]
    fn rollup_output_may_be_named_values() {
        let dir = TempDir::new().unwrap();
        let root = dir.path();
        std::fs::write(
            root.join(".markdownvdb.schema.yml"),
            r#"scopes:
  clients:
    fields:
      invoices:
        field_type: relation
        target: invoices
      values:
        field_type: rollup
        relation_field: invoices
        target_field: amount
        formula: values.reduce((sum, value) => sum + value, 0)
        result_type: number
"#,
        )
        .unwrap();
        let index = index(root);
        add_file(root, &index, "invoices/a.md", "---\namount: 2\n---\nA\n");
        add_file(root, &index, "invoices/b.md", "---\namount: 3\n---\nB\n");
        add_file(
            root,
            &index,
            "clients/acme.md",
            "---\ninvoices: ['[[a]]', '[[b]]']\n---\nAcme\n",
        );

        let reports = ModuleRunner::builtins()
            .run(root, &index, &ModuleEvent::FullIngest)
            .unwrap();
        assert!(
            reports[1].diagnostics.is_empty(),
            "{:?}",
            reports[1].diagnostics
        );
        assert_eq!(
            parsed_frontmatter(root, "clients/acme.md")["values"],
            serde_json::json!(5)
        );
    }

    #[test]
    fn failed_formula_target_marks_rollup_dependency_failed() {
        let dir = TempDir::new().unwrap();
        let root = dir.path();
        std::fs::write(
            root.join(".markdownvdb.schema.yml"),
            r#"scopes:
  invoices:
    fields:
      client:
        field_type: relation
        target: clients
      total:
        field_type: formula
        formula: price * quantity
        result_type: number
  clients:
    fields:
      invoice_total:
        field_type: rollup
        relation_field: client
        target_field: total
        relation_direction: incoming
        relation_scope: invoices
        formula: values.reduce((sum, value) => sum + value, 0)
        result_type: number
"#,
        )
        .unwrap();
        let index = index(root);
        add_file(
            root,
            &index,
            "clients/acme.md",
            "---\ninvoice_total: 999\n---\nAcme\n",
        );
        seed_materialized(
            &index,
            "clients/acme.md",
            "invoice_total",
            JsonValue::from(999),
        );
        add_file(
            root,
            &index,
            "invoices/broken.md",
            "---\nclient: '[[clients/acme]]'\nprice: 10\n---\nBroken\n",
        );

        let reports = ModuleRunner::builtins()
            .run(root, &index, &ModuleEvent::FullIngest)
            .unwrap();
        assert_eq!(reports[0].module, crate::formula::FORMULA_MODULE_ID);
        assert_eq!(reports[0].diagnostics.len(), 1);
        assert_eq!(reports[1].diagnostics.len(), 1);
        assert_eq!(reports[1].diagnostics[0].field, "invoice_total");
        assert_eq!(reports[1].diagnostics[0].code, "dependency_failed");
        assert!(reports[1].diagnostics[0]
            .message
            .contains("invoices/broken.md.total"));
        assert!(parsed_frontmatter_or_empty(root, "clients/acme.md")
            .get("invoice_total")
            .is_none());
    }

    #[test]
    fn malformed_incoming_relation_fails_instead_of_undercounting() {
        let dir = TempDir::new().unwrap();
        let root = dir.path();
        std::fs::write(
            root.join(".markdownvdb.schema.yml"),
            r#"scopes:
  invoices:
    fields:
      client:
        field_type: relation
        target: clients
  clients:
    fields:
      invoice_total:
        field_type: rollup
        relation_field: client
        target_field: amount
        relation_direction: incoming
        relation_scope: invoices
        formula: values.reduce((sum, value) => sum + value, 0)
        result_type: number
"#,
        )
        .unwrap();
        let index = index(root);
        add_file(
            root,
            &index,
            "clients/acme.md",
            "---\ninvoice_total: 999\n---\nAcme\n",
        );
        seed_materialized(
            &index,
            "clients/acme.md",
            "invoice_total",
            JsonValue::from(999),
        );
        add_file(
            root,
            &index,
            "invoices/valid.md",
            "---\nclient: '[[clients/acme]]'\namount: 10\n---\nValid\n",
        );
        add_file(
            root,
            &index,
            "invoices/malformed.md",
            "---\nclient: ['[[clients/acme]]', 42]\namount: 100\n---\nMalformed\n",
        );

        let reports = ModuleRunner::builtins()
            .run(root, &index, &ModuleEvent::FullIngest)
            .unwrap();
        assert_eq!(reports[1].diagnostics.len(), 1);
        assert_eq!(reports[1].diagnostics[0].field, "invoice_total");
        assert_eq!(reports[1].diagnostics[0].code, "invalid_relation");
        assert!(parsed_frontmatter_or_empty(root, "clients/acme.md")
            .get("invoice_total")
            .is_none());
        let state = index.get_computed_fields("clients/acme.md").unwrap();
        assert_eq!(
            state["invoice_total"].diagnostic.as_ref().unwrap().code,
            "invalid_relation"
        );
    }

    #[test]
    fn missing_target_field_fails_closed_and_removes_old_materialization() {
        let dir = TempDir::new().unwrap();
        let root = dir.path();
        std::fs::write(
            root.join(".markdownvdb.schema.yml"),
            r#"scopes:
  contacts:
    fields:
      domain:
        field_type: lookup
        relation_field: client
        target_field: domain
"#,
        )
        .unwrap();
        let index = index(root);
        add_file(root, &index, "clients/a.md", "---\nname: A\n---\nA\n");
        add_file(
            root,
            &index,
            "contacts/a.md",
            "---\nclient: '[[clients/a]]'\ndomain: stale.example\n---\nContact\n",
        );
        index
            .replace_computed_fields(
                "contacts/a.md",
                HashMap::from([(
                    "domain".to_string(),
                    ComputedFieldEntry {
                        module: LOOKUP_ROLLUP_MODULE_ID.to_string(),
                        definition_fingerprint: "old".to_string(),
                        input_fingerprint: None,
                        dependency_snapshot: ComputedDependencySnapshot::default(),
                        value_json: Some("\"stale.example\"".to_string()),
                        materialized_value_json: Some("\"stale.example\"".to_string()),
                        diagnostic: None,
                    },
                )]),
            )
            .unwrap();
        index.save().unwrap();

        let reports = ModuleRunner::builtins()
            .run(root, &index, &ModuleEvent::FullIngest)
            .unwrap();
        assert_eq!(reports[1].diagnostics[0].code, "target_field_missing");
        let contact = parsed_frontmatter(root, "contacts/a.md");
        assert!(contact.get("domain").is_none());
        assert!(
            index.get_computed_fields("contacts/a.md").unwrap()["domain"]
                .value_json
                .is_none()
        );
    }

    #[test]
    fn unresolved_lookup_persists_absent_resolution_candidates() {
        let dir = TempDir::new().unwrap();
        let root = dir.path();
        std::fs::write(
            root.join(".markdownvdb.schema.yml"),
            r#"scopes:
  contacts:
    fields:
      client:
        field_type: relation
        target: clients
      domain:
        field_type: lookup
        relation_field: client
        target_field: domain
"#,
        )
        .unwrap();
        let index = index(root);
        add_file(
            root,
            &index,
            "contacts/a.md",
            "---\nclient: '[[missing]]'\n---\nContact\n",
        );

        let reports = ModuleRunner::builtins()
            .run(root, &index, &ModuleEvent::FullIngest)
            .unwrap();
        assert_eq!(reports[1].diagnostics[0].code, "relation_unresolved");

        let state = index.get_computed_fields("contacts/a.md").unwrap();
        let snapshot = &state["domain"].dependency_snapshot;
        let absent: Vec<_> = snapshot
            .paths
            .iter()
            .filter(|(_, state)| !state.exists)
            .collect();
        assert!(!absent.is_empty());
        assert!(absent.iter().all(|(_, state)| state.content_hash.is_none()));
    }

    #[test]
    fn lookup_cycles_are_reported_for_every_participant() {
        let dir = TempDir::new().unwrap();
        let root = dir.path();
        std::fs::write(
            root.join(".markdownvdb.schema.yml"),
            r#"scopes:
  docs:
    fields:
      mirrored:
        field_type: lookup
        relation_field: peer
        target_field: mirrored
"#,
        )
        .unwrap();
        let index = index(root);
        add_file(
            root,
            &index,
            "docs/a.md",
            "---\npeer: '[[docs/b]]'\n---\nA\n",
        );
        add_file(
            root,
            &index,
            "docs/b.md",
            "---\npeer: '[[docs/a]]'\n---\nB\n",
        );

        let reports = ModuleRunner::builtins()
            .run(root, &index, &ModuleEvent::FullIngest)
            .unwrap();
        let diagnostics = &reports[1].diagnostics;
        assert_eq!(diagnostics.len(), 2);
        assert!(diagnostics
            .iter()
            .all(|diagnostic| diagnostic.code == "dependency_cycle"));
        assert!(parsed_frontmatter(root, "docs/a.md")
            .get("mirrored")
            .is_none());
        assert!(parsed_frontmatter(root, "docs/b.md")
            .get("mirrored")
            .is_none());
    }

    #[test]
    fn explicit_non_relation_source_field_fails_with_type_diagnostic() {
        let dir = TempDir::new().unwrap();
        let root = dir.path();
        std::fs::write(
            root.join(".markdownvdb.schema.yml"),
            r#"scopes:
  contacts:
    fields:
      client:
        field_type: string
      domain:
        field_type: lookup
        relation_field: client
        target_field: domain
"#,
        )
        .unwrap();
        let index = index(root);
        add_file(
            root,
            &index,
            "clients/a.md",
            "---\ndomain: a.example\n---\nA\n",
        );
        add_file(
            root,
            &index,
            "contacts/a.md",
            "---\nclient: '[[clients/a]]'\n---\nContact\n",
        );

        let reports = ModuleRunner::builtins()
            .run(root, &index, &ModuleEvent::FullIngest)
            .unwrap();
        assert_eq!(reports[1].diagnostics.len(), 1);
        assert_eq!(reports[1].diagnostics[0].code, "relation_field_type");
        assert!(parsed_frontmatter(root, "contacts/a.md")
            .get("domain")
            .is_none());
    }

    #[test]
    fn released_lookup_tombstone_is_an_ordinary_target_and_cache_input() {
        let dir = TempDir::new().unwrap();
        let root = dir.path();
        std::fs::write(
            root.join(".markdownvdb.schema.yml"),
            r#"scopes:
  contacts:
    fields:
      copied:
        field_type: lookup
        relation_field: client
        target_field: legacy
"#,
        )
        .unwrap();
        let index = index(root);
        add_file(
            root,
            &index,
            "clients/a.md",
            "---\nlegacy: 7\n---\nClient\n",
        );
        add_file(
            root,
            &index,
            "contacts/a.md",
            "---\nclient: '[[clients/a]]'\n---\nContact\n",
        );
        index
            .replace_computed_fields(
                "clients/a.md",
                HashMap::from([(
                    "legacy".to_string(),
                    ComputedFieldEntry {
                        module: LOOKUP_ROLLUP_MODULE_ID.to_string(),
                        definition_fingerprint: String::new(),
                        input_fingerprint: None,
                        dependency_snapshot: ComputedDependencySnapshot::default(),
                        value_json: None,
                        materialized_value_json: None,
                        diagnostic: Some(ComputedFieldDiagnostic {
                            module: LOOKUP_ROLLUP_MODULE_ID.to_string(),
                            field: "legacy".to_string(),
                            code: "invalid_schema".to_string(),
                            message: "old definition was cleared".to_string(),
                            span_start: None,
                            span_end: None,
                        }),
                    },
                )]),
            )
            .unwrap();
        index.save().unwrap();
        assert!(
            !index.get_computed_fields("clients/a.md").unwrap()["legacy"].has_materialized_proof()
        );

        let runner = ModuleRunner::builtins();
        let event = ModuleEvent::ManualRun {
            scope: Some("contacts".to_string()),
        };
        let report = runner
            .run_one(LOOKUP_ROLLUP_MODULE_ID, root, &index, &event)
            .unwrap();
        assert!(report.diagnostics.is_empty(), "{:?}", report.diagnostics);
        assert_eq!(parsed_frontmatter(root, "contacts/a.md")["copied"], 7);

        // Keeping the released tombstone in the out-of-scope target exercises
        // both raw target reads and the cache input fingerprint.
        std::fs::write(root.join("clients/a.md"), "---\nlegacy: 8\n---\nClient\n").unwrap();
        let changed = crate::parser::parse_markdown_file(root, Path::new("clients/a.md")).unwrap();
        index.refresh_source_metadata(&changed).unwrap();
        index.save().unwrap();

        let report = runner
            .run_one(LOOKUP_ROLLUP_MODULE_ID, root, &index, &event)
            .unwrap();
        assert!(report.diagnostics.is_empty(), "{:?}", report.diagnostics);
        assert_eq!(parsed_frontmatter(root, "contacts/a.md")["copied"], 8);
        assert_eq!(parsed_frontmatter(root, "clients/a.md")["legacy"], 8);
    }

    #[test]
    fn removing_definition_cleans_owned_value_and_schema_column() {
        let dir = TempDir::new().unwrap();
        let root = dir.path();
        let overlay_path = root.join(".markdownvdb.schema.yml");
        std::fs::write(
            &overlay_path,
            r#"scopes:
  contacts:
    fields:
      client:
        field_type: relation
        target: clients
      client_domain:
        field_type: lookup
        relation_field: client
        target_field: domain
"#,
        )
        .unwrap();
        let index = index(root);
        add_file(
            root,
            &index,
            "clients/acme.md",
            "---\ndomain: acme.example\n---\nAcme\n",
        );
        add_file(
            root,
            &index,
            "contacts/alice.md",
            "---\nclient: '[[clients/acme]]'\nordinary: kept\n---\nAlice\n",
        );

        ModuleRunner::builtins()
            .run(root, &index, &ModuleEvent::FullIngest)
            .unwrap();
        assert_eq!(
            parsed_frontmatter(root, "contacts/alice.md")["client_domain"],
            "acme.example"
        );

        std::fs::write(
            &overlay_path,
            r#"scopes:
  contacts:
    fields:
      client:
        field_type: relation
        target: clients
"#,
        )
        .unwrap();
        let reports = ModuleRunner::builtins()
            .run(root, &index, &ModuleEvent::FullIngest)
            .unwrap();
        assert!(
            reports[1].diagnostics.is_empty(),
            "{:?}",
            reports[1].diagnostics
        );

        let contact = parsed_frontmatter(root, "contacts/alice.md");
        assert!(contact.get("client_domain").is_none());
        assert_eq!(contact["ordinary"], "kept");
        assert!(!index
            .get_computed_fields("contacts/alice.md")
            .unwrap()
            .contains_key("client_domain"));
        let global_schema = index.get_schema().unwrap();
        assert!(!global_schema
            .fields
            .iter()
            .any(|field| field.name == "client_domain"));
        let contacts_schema = index.get_scoped_schema("contacts").unwrap();
        assert!(!contacts_schema
            .schema
            .fields
            .iter()
            .any(|field| field.name == "client_domain"));
    }

    #[test]
    fn scoped_computed_name_preserves_ordinary_values_in_other_scopes() {
        let dir = TempDir::new().unwrap();
        let root = dir.path();
        std::fs::write(
            root.join(".markdownvdb.schema.yml"),
            r#"scopes:
  contacts:
    fields:
      client:
        field_type: relation
        target: clients
      shared:
        field_type: lookup
        relation_field: client
        target_field: domain
"#,
        )
        .unwrap();
        let index = index(root);
        add_file(
            root,
            &index,
            "clients/acme.md",
            "---\ndomain: computed.example\n---\nAcme\n",
        );
        add_file(
            root,
            &index,
            "contacts/alice.md",
            "---\nclient: '[[acme]]'\n---\nAlice\n",
        );
        add_file(
            root,
            &index,
            "notes/ordinary.md",
            "---\nshared: ordinary.example\n---\nOrdinary\n",
        );

        ModuleRunner::builtins()
            .run(root, &index, &ModuleEvent::FullIngest)
            .unwrap();

        let global = index.get_schema().unwrap();
        let shared = global
            .fields
            .iter()
            .find(|field| field.name == "shared")
            .expect("ordinary cross-scope field remains in the global schema");
        assert_eq!(shared.field_type, FieldType::String);
        assert_eq!(shared.occurrence_count, 1);
        assert_eq!(shared.sample_values, vec!["ordinary.example"]);

        let contacts = index.get_scoped_schema("contacts").unwrap();
        assert_eq!(
            contacts
                .schema
                .fields
                .iter()
                .find(|field| field.name == "shared")
                .unwrap()
                .field_type,
            FieldType::Lookup
        );
        let notes = index.get_scoped_schema("notes").unwrap();
        assert_eq!(
            notes
                .schema
                .fields
                .iter()
                .find(|field| field.name == "shared")
                .unwrap()
                .field_type,
            FieldType::String
        );
    }

    #[test]
    fn identical_recomputation_emits_no_derived_field_patch() {
        let dir = TempDir::new().unwrap();
        let root = dir.path();
        std::fs::write(
            root.join(".markdownvdb.schema.yml"),
            r#"scopes:
  contacts:
    fields:
      client:
        field_type: relation
        target: clients
      domain:
        field_type: lookup
        relation_field: client
        target_field: domain
"#,
        )
        .unwrap();
        let index = index(root);
        add_file(
            root,
            &index,
            "clients/a.md",
            "---\ndomain: a.example\n---\nA\n",
        );
        add_file(
            root,
            &index,
            "contacts/a.md",
            "---\nclient: '[[a]]'\n---\nContact\n",
        );
        ModuleRunner::builtins()
            .run(root, &index, &ModuleEvent::FullIngest)
            .unwrap();

        let files = index.get_all_files();
        let schema = index.get_schema();
        let scoped_schemas = index.get_scoped_schemas();
        let context = ModuleContext {
            project_root: root,
            files: &files,
            schema: schema.as_ref(),
            scoped_schemas: scoped_schemas.as_deref(),
        };
        let execution = LookupRollupModule::default()
            .run(&context, &ModuleEvent::FullIngest)
            .unwrap();

        assert_eq!(execution.files_evaluated, 1);
        assert_eq!(execution.fields_updated, 0);
        assert!(execution.derived_field_patches.is_empty());
    }

    #[test]
    fn fingerprints_track_only_selected_dependencies() {
        let dir = TempDir::new().unwrap();
        let root = dir.path();
        std::fs::write(
            root.join(".markdownvdb.schema.yml"),
            r#"scopes:
  contacts:
    fields:
      client:
        field_type: relation
        target: clients
      domain:
        field_type: lookup
        relation_field: client
        target_field: domain
"#,
        )
        .unwrap();
        let index = index(root);
        add_file(
            root,
            &index,
            "clients/a.md",
            "---\ndomain: a.example\nnote: first\n---\nA\n",
        );
        add_file(
            root,
            &index,
            "clients/b.md",
            "---\ndomain: b.example\nnote: first\n---\nB\n",
        );
        add_file(
            root,
            &index,
            "contacts/a.md",
            "---\nclient: '[[clients/a]]'\n---\nA\n",
        );
        add_file(
            root,
            &index,
            "contacts/b.md",
            "---\nclient: '[[clients/b]]'\n---\nB\n",
        );
        let runner = ModuleRunner::builtins();
        runner.run(root, &index, &ModuleEvent::FullIngest).unwrap();
        let a_before = index.get_computed_fields("contacts/a.md").unwrap()["domain"]
            .input_fingerprint
            .clone();
        let b_before = index.get_computed_fields("contacts/b.md").unwrap()["domain"]
            .input_fingerprint
            .clone();

        std::fs::write(
            root.join("clients/b.md"),
            "---\ndomain: b.example\nnote: unrelated edit\n---\nB\n",
        )
        .unwrap();
        let changed = crate::parser::parse_markdown_file(root, Path::new("clients/b.md")).unwrap();
        index.refresh_source_metadata(&changed).unwrap();
        index.save().unwrap();
        let reports = runner.run(root, &index, &ModuleEvent::FullIngest).unwrap();
        assert_eq!(reports[1].fields_updated, 0);
        assert_eq!(
            index.get_computed_fields("contacts/a.md").unwrap()["domain"].input_fingerprint,
            a_before
        );
        assert_eq!(
            index.get_computed_fields("contacts/b.md").unwrap()["domain"].input_fingerprint,
            b_before
        );

        std::fs::write(
            root.join("clients/b.md"),
            "---\ndomain: changed.example\nnote: unrelated edit\n---\nB\n",
        )
        .unwrap();
        let changed = crate::parser::parse_markdown_file(root, Path::new("clients/b.md")).unwrap();
        index.refresh_source_metadata(&changed).unwrap();
        index.save().unwrap();
        let reports = runner.run(root, &index, &ModuleEvent::FullIngest).unwrap();
        assert_eq!(reports[1].fields_updated, 1);
        assert_eq!(
            parsed_frontmatter(root, "contacts/b.md")["domain"],
            "changed.example"
        );
        assert_eq!(
            index.get_computed_fields("contacts/a.md").unwrap()["domain"].input_fingerprint,
            a_before
        );
        assert_ne!(
            index.get_computed_fields("contacts/b.md").unwrap()["domain"].input_fingerprint,
            b_before
        );
    }
}
