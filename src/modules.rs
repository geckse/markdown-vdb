//! Built-in derived-data modules and their ingest/watch hook runner.
//!
//! Modules are compiled into mdvdb. They receive an index change event and
//! return declarative source/index patches. Only the runner may atomically
//! materialize an allowed patch into Markdown.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::Path;
use std::str::FromStr;
use std::time::Instant;

use serde::Serialize;
use serde_json::{Map as JsonMap, Value as JsonValue};

use crate::formula::{
    FormulaDefinition, FormulaDiagnostic, FormulaEngine, FormulaProgram, FORMULA_MODULE_ID,
};
use crate::index::state::Index;
use crate::index::types::{ComputedFieldDiagnostic, ComputedFieldEntry, StoredFile};
use crate::schema::{FieldType, FormulaResultType, OverlaySchema, Schema};

/// Change notification delivered to every always-on module.
#[derive(Debug, Clone)]
pub enum ModuleEvent {
    FullIngest,
    FilesChanged {
        upserted: Vec<String>,
        removed: Vec<String>,
        renamed: Vec<(String, String)>,
    },
    SchemaChanged,
    ManualRun {
        scope: Option<String>,
    },
}

impl ModuleEvent {
    pub fn name(&self) -> &'static str {
        match self {
            Self::FullIngest => "full_ingest",
            Self::FilesChanged { .. } => "files_changed",
            Self::SchemaChanged => "schema_changed",
            Self::ManualRun { .. } => "manual_run",
        }
    }
}

/// Static information surfaced by `mdvdb modules list`.
#[derive(Debug, Clone, Serialize)]
pub struct ModuleDescriptor {
    pub id: String,
    pub name: String,
    pub version: u32,
    pub always_on: bool,
    pub hooks: Vec<String>,
}

/// One formula/module diagnostic with optional source and document location.
#[derive(Debug, Clone, Serialize)]
pub struct ModuleDiagnostic {
    pub path: Option<String>,
    pub module: String,
    pub field: String,
    pub code: String,
    pub message: String,
    pub span_start: Option<usize>,
    pub span_end: Option<usize>,
}

/// Raw outcome returned by a module implementation.
#[derive(Debug, Clone, Default)]
pub struct ModuleExecution {
    pub files_evaluated: usize,
    pub fields_updated: usize,
    pub diagnostics: Vec<ModuleDiagnostic>,
    pub derived_field_patches: Vec<DerivedFieldPatch>,
    pub schema_patch: Option<ModuleSchemaPatch>,
}

/// Complete derived-field replacement for one indexed file.
#[derive(Debug, Clone)]
pub struct DerivedFieldPatch {
    pub path: String,
    /// Full source hash used to evaluate this patch (filesystem CAS guard).
    pub expected_content_hash: String,
    pub fields: HashMap<String, ComputedFieldEntry>,
    /// Successful materialized values, encoded without an `f64` round-trip.
    pub frontmatter_set: BTreeMap<String, JsonValue>,
    /// Previously/currently owned fields to remove before applying `set`.
    pub frontmatter_unset: BTreeSet<String>,
}

/// Schema metadata derived by a module from raw frontmatter and definitions.
#[derive(Debug, Clone)]
pub struct ModuleSchemaPatch {
    pub schema: Option<Schema>,
    pub scoped_schemas: Option<Vec<crate::schema::ScopedSchema>>,
}

/// Stable report included in ingest/watch output and manual module commands.
#[derive(Debug, Clone, Serialize)]
pub struct ModuleReport {
    pub module: String,
    pub event: String,
    pub files_evaluated: usize,
    pub fields_updated: usize,
    pub diagnostics: Vec<ModuleDiagnostic>,
    pub duration_ms: u64,
}

/// Restricted host context supplied to a compiled-in module.
pub struct ModuleContext<'a> {
    pub project_root: &'a Path,
    pub files: &'a HashMap<String, StoredFile>,
    pub schema: Option<&'a Schema>,
    pub scoped_schemas: Option<&'a [crate::schema::ScopedSchema]>,
}

/// A built-in module. Implementations receive a read-only state snapshot and
/// return patches; only [`ModuleRunner`] mutates the index.
pub trait Module: Send + Sync {
    fn descriptor(&self) -> ModuleDescriptor;

    fn run(
        &self,
        context: &ModuleContext<'_>,
        event: &ModuleEvent,
    ) -> crate::Result<ModuleExecution>;
}

/// Runs all registered modules in deterministic registration order.
pub struct ModuleRunner {
    modules: Vec<Box<dyn Module>>,
}

struct DerivedStateSnapshot {
    files: HashMap<String, StoredFile>,
    schema: Option<Schema>,
    scoped_schemas: Option<Vec<crate::schema::ScopedSchema>>,
}

impl DerivedStateSnapshot {
    fn capture(index: &Index) -> Self {
        Self {
            files: index.get_all_files(),
            schema: index.get_schema(),
            scoped_schemas: index.get_scoped_schemas(),
        }
    }

    fn context<'a>(&'a self, project_root: &'a Path) -> ModuleContext<'a> {
        ModuleContext {
            project_root,
            files: &self.files,
            schema: self.schema.as_ref(),
            scoped_schemas: self.scoped_schemas.as_deref(),
        }
    }

    fn restore_failed_module(
        &self,
        project_root: &Path,
        index: &Index,
        module: &str,
        message: &str,
    ) {
        index.set_schema(self.schema.clone());
        index.set_scoped_schemas(self.scoped_schemas.clone());
        for (path, file) in &self.files {
            let mut fields = file.computed_fields.clone();
            let owned_fields: BTreeSet<String> = fields
                .iter()
                .filter(|(_, entry)| entry.module == module)
                .map(|(field, _)| field.clone())
                .collect();
            for (field, entry) in &mut fields {
                if entry.module != module {
                    continue;
                }
                entry.value_json = None;
                entry.diagnostic = Some(ComputedFieldDiagnostic {
                    module: module.to_string(),
                    field: field.clone(),
                    code: "module_error".to_string(),
                    message: message.to_string(),
                    span_start: None,
                    span_end: None,
                });
            }

            if module == FORMULA_MODULE_ID && !owned_fields.is_empty() {
                if let Ok(writeback) = crate::frontmatter_write::apply_frontmatter_patch(
                    project_root,
                    Path::new(path),
                    &file.content_hash,
                    &BTreeMap::new(),
                    &owned_fields,
                ) {
                    if index
                        .apply_module_source_state(
                            &file.content_hash,
                            &writeback.file,
                            fields.clone(),
                        )
                        .is_ok()
                    {
                        continue;
                    }
                }
            }
            let _ = index.replace_computed_fields(path, fields);
        }
    }
}

impl ModuleRunner {
    pub fn new(modules: Vec<Box<dyn Module>>) -> Self {
        Self { modules }
    }

    /// Construct the deterministic registry of compiled-in, always-on modules.
    pub fn builtins() -> Self {
        Self::new(vec![Box::new(FormulaModule::default())])
    }

    pub fn descriptors(&self) -> Vec<ModuleDescriptor> {
        self.modules
            .iter()
            .map(|module| module.descriptor())
            .collect()
    }

    fn apply_execution(
        project_root: &Path,
        index: &Index,
        execution: &mut ModuleExecution,
    ) -> crate::Result<()> {
        for patch in &mut execution.derived_field_patches {
            let relative_path = Path::new(&patch.path);
            match crate::frontmatter_write::apply_frontmatter_patch(
                project_root,
                relative_path,
                &patch.expected_content_hash,
                &patch.frontmatter_set,
                &patch.frontmatter_unset,
            ) {
                Ok(writeback) => {
                    index.apply_module_source_state(
                        &patch.expected_content_hash,
                        &writeback.file,
                        patch.fields.clone(),
                    )?;
                }
                Err(error) => {
                    let code = if matches!(error, crate::Error::SourceChanged { .. }) {
                        "source_changed"
                    } else {
                        "writeback_failed"
                    };
                    let message = error.to_string();
                    let affected: BTreeSet<String> = patch
                        .frontmatter_unset
                        .iter()
                        .chain(patch.frontmatter_set.keys())
                        .cloned()
                        .collect();
                    for field in affected {
                        let definition_fingerprint = patch
                            .fields
                            .get(&field)
                            .map(|entry| entry.definition_fingerprint.clone())
                            .unwrap_or_default();
                        let diagnostic = ComputedFieldDiagnostic {
                            module: FORMULA_MODULE_ID.to_string(),
                            field: field.clone(),
                            code: code.to_string(),
                            message: message.clone(),
                            span_start: None,
                            span_end: None,
                        };
                        patch.fields.insert(
                            field.clone(),
                            ComputedFieldEntry {
                                module: FORMULA_MODULE_ID.to_string(),
                                definition_fingerprint,
                                value_json: None,
                                diagnostic: Some(diagnostic),
                            },
                        );
                        execution.diagnostics.push(ModuleDiagnostic {
                            path: Some(patch.path.clone()),
                            module: FORMULA_MODULE_ID.to_string(),
                            field,
                            code: code.to_string(),
                            message: message.clone(),
                            span_start: None,
                            span_end: None,
                        });
                    }
                    index.replace_computed_fields(&patch.path, patch.fields.clone())?;
                }
            }
        }

        if let Some(patch) = &mut execution.schema_patch {
            let files = index.get_all_files();
            if let Some(schema) = &mut patch.schema {
                FormulaModule::refresh_one_schema(schema, files.values());
            }
            if let Some(scoped_schemas) = &mut patch.scoped_schemas {
                for scoped in scoped_schemas {
                    FormulaModule::refresh_one_schema(
                        &mut scoped.schema,
                        files.iter().filter_map(|(path, file)| {
                            FormulaModule::path_matches_scope(path, &scoped.scope).then_some(file)
                        }),
                    );
                }
            }
            index.set_schema(patch.schema.clone());
            index.set_scoped_schemas(patch.scoped_schemas.clone());
        }
        Ok(())
    }

    fn failure_execution(module: &str, message: String) -> ModuleExecution {
        ModuleExecution {
            diagnostics: vec![ModuleDiagnostic {
                path: None,
                module: module.to_string(),
                field: String::new(),
                code: "module_error".to_string(),
                message,
                span_start: None,
                span_end: None,
            }],
            ..ModuleExecution::default()
        }
    }

    fn execute(
        module: &dyn Module,
        project_root: &Path,
        index: &Index,
        event: &ModuleEvent,
    ) -> ModuleReport {
        let descriptor = module.descriptor();
        let started = Instant::now();
        let snapshot = DerivedStateSnapshot::capture(index);
        let context = snapshot.context(project_root);
        let execution = match module.run(&context, event) {
            Ok(mut execution) => match Self::apply_execution(project_root, index, &mut execution) {
                Ok(()) => execution,
                Err(error) => {
                    let message = error.to_string();
                    snapshot.restore_failed_module(project_root, index, &descriptor.id, &message);
                    Self::failure_execution(&descriptor.id, message)
                }
            },
            Err(error) => {
                let message = error.to_string();
                snapshot.restore_failed_module(project_root, index, &descriptor.id, &message);
                Self::failure_execution(&descriptor.id, message)
            }
        };

        ModuleReport {
            module: descriptor.id,
            event: event.name().to_string(),
            files_evaluated: execution.files_evaluated,
            fields_updated: execution.fields_updated,
            diagnostics: execution.diagnostics,
            duration_ms: started.elapsed().as_millis() as u64,
        }
    }

    pub fn run(
        &self,
        project_root: &Path,
        index: &Index,
        event: &ModuleEvent,
    ) -> Vec<ModuleReport> {
        self.modules
            .iter()
            .map(|module| Self::execute(module.as_ref(), project_root, index, event))
            .collect()
    }

    pub fn run_one(
        &self,
        id: &str,
        project_root: &Path,
        index: &Index,
        event: &ModuleEvent,
    ) -> Option<ModuleReport> {
        let module = self
            .modules
            .iter()
            .find(|module| module.descriptor().id == id)?;
        Some(Self::execute(module.as_ref(), project_root, index, event))
    }
}

/// Always-on calculated-field module.
#[derive(Debug, Default)]
pub struct FormulaModule {
    engine: FormulaEngine,
}

impl FormulaModule {
    fn definitions_for_path(overlay: &OverlaySchema, path: &str) -> Vec<FormulaDefinition> {
        let mut definitions: Vec<FormulaDefinition> =
            Schema::resolve_overlay_for_path(overlay, Some(path))
                .into_iter()
                .filter_map(|(name, field)| {
                    let is_formula = field
                        .field_type
                        .as_deref()
                        .is_some_and(|kind| kind.eq_ignore_ascii_case("formula"));
                    if !is_formula {
                        return None;
                    }
                    let formula = field.formula?;
                    let result_type =
                        FormulaResultType::from_str(field.result_type.as_deref()?).ok()?;
                    Some(FormulaDefinition::new(name, formula, result_type))
                })
                .collect();
        definitions.sort_by(|left, right| left.name.cmp(&right.name));
        definitions
    }

    fn input_fields(path: &str, file: &StoredFile) -> JsonMap<String, JsonValue> {
        let mut fields = file
            .frontmatter
            .as_deref()
            .and_then(|raw| serde_json::from_str::<JsonValue>(raw).ok())
            .and_then(|value| value.as_object().cloned())
            .unwrap_or_default();

        fields.insert("path".to_string(), JsonValue::String(path.to_string()));
        let derived_title = fields
            .get("title")
            .and_then(JsonValue::as_str)
            .filter(|title| !title.trim().is_empty())
            .map(str::to_string)
            .unwrap_or_else(|| {
                Path::new(path)
                    .file_stem()
                    .and_then(|stem| stem.to_str())
                    .filter(|stem| !stem.is_empty())
                    .unwrap_or(path)
                    .to_string()
            });
        fields.insert("title".to_string(), JsonValue::String(derived_title));
        fields
    }

    fn has_formula_state(file: &StoredFile) -> bool {
        file.computed_fields
            .values()
            .any(|entry| entry.module == FORMULA_MODULE_ID)
    }

    fn path_matches_scope(path: &str, scope: &str) -> bool {
        crate::path_util::path_is_in_scope(path, scope)
    }

    fn needs_recompute(&self, overlay: &OverlaySchema, path: &str, file: &StoredFile) -> bool {
        let definitions = Self::definitions_for_path(overlay, path);
        let expected: BTreeSet<&str> = definitions
            .iter()
            .map(|definition| definition.name.as_str())
            .collect();
        let existing: BTreeMap<&str, &ComputedFieldEntry> = file
            .computed_fields
            .iter()
            .filter(|(_, entry)| entry.module == FORMULA_MODULE_ID)
            .map(|(field, entry)| (field.as_str(), entry))
            .collect();

        if definitions.is_empty() {
            return !existing.is_empty();
        }
        if existing.len() != expected.len()
            || existing.keys().any(|field| !expected.contains(field))
            || expected.iter().any(|field| !existing.contains_key(field))
        {
            return true;
        }

        let fingerprint = self.engine.compile(definitions).fingerprint().to_string();
        existing
            .values()
            .any(|entry| entry.definition_fingerprint != fingerprint)
    }

    fn stored_diagnostic(diagnostic: &FormulaDiagnostic) -> ComputedFieldDiagnostic {
        ComputedFieldDiagnostic {
            module: diagnostic.module.clone(),
            field: diagnostic.field.clone(),
            code: diagnostic.code.clone(),
            message: diagnostic.message.clone(),
            span_start: diagnostic.span.as_ref().map(|span| span.start as usize),
            span_end: diagnostic.span.as_ref().map(|span| span.end as usize),
        }
    }

    fn report_diagnostic(path: &str, diagnostic: &FormulaDiagnostic) -> ModuleDiagnostic {
        ModuleDiagnostic {
            path: Some(path.to_string()),
            module: diagnostic.module.clone(),
            field: diagnostic.field.clone(),
            code: diagnostic.code.clone(),
            message: diagnostic.message.clone(),
            span_start: diagnostic.span.as_ref().map(|span| span.start as usize),
            span_end: diagnostic.span.as_ref().map(|span| span.end as usize),
        }
    }

    fn evaluate_file(
        &self,
        path: &str,
        file: &StoredFile,
        program: &FormulaProgram,
        execution: &mut ModuleExecution,
    ) {
        let mut inputs = Self::input_fields(path, file);
        for definition in program.definitions() {
            inputs.remove(&definition.name);
        }
        for (field, entry) in &file.computed_fields {
            if entry.module == FORMULA_MODULE_ID {
                inputs.remove(field);
            }
        }
        let evaluation = program.evaluate(&inputs);
        let mut entries = file.computed_fields.clone();
        let mut frontmatter_unset: BTreeSet<String> = entries
            .iter()
            .filter(|(_, entry)| entry.module == FORMULA_MODULE_ID)
            .map(|(field, _)| field.clone())
            .collect();
        entries.retain(|_, entry| entry.module != FORMULA_MODULE_ID);
        let mut frontmatter_set = BTreeMap::new();

        for definition in program.definitions() {
            let diagnostic = evaluation.errors.get(&definition.name);
            frontmatter_unset.insert(definition.name.clone());
            let value = evaluation
                .values
                .get(&definition.name)
                .and_then(|value| value.to_json().ok());
            if let Some(value) = &value {
                frontmatter_set.insert(definition.name.clone(), value.clone());
            }
            let value_json = value.and_then(|value| serde_json::to_string(&value).ok());

            if let Some(diagnostic) = diagnostic {
                execution
                    .diagnostics
                    .push(Self::report_diagnostic(path, diagnostic));
            }
            entries.insert(
                definition.name.clone(),
                ComputedFieldEntry {
                    module: FORMULA_MODULE_ID.to_string(),
                    definition_fingerprint: program.fingerprint().to_string(),
                    value_json,
                    diagnostic: diagnostic.map(Self::stored_diagnostic),
                },
            );
        }

        execution.fields_updated += program.definitions().count();
        execution.derived_field_patches.push(DerivedFieldPatch {
            path: path.to_string(),
            expected_content_hash: file.content_hash.clone(),
            fields: entries,
            frontmatter_set,
            frontmatter_unset,
        });
    }

    fn persist_schema_error(
        &self,
        context: &ModuleContext<'_>,
        paths: &[String],
        message: &str,
        execution: &mut ModuleExecution,
    ) {
        for path in paths {
            let Some(file) = context.files.get(path) else {
                continue;
            };
            execution.files_evaluated += 1;
            let mut entries = file.computed_fields.clone();
            let previous_fields: Vec<String> = entries
                .iter()
                .filter(|(_, entry)| entry.module == FORMULA_MODULE_ID)
                .map(|(field, _)| field.clone())
                .collect();
            entries.retain(|_, entry| entry.module != FORMULA_MODULE_ID);
            let frontmatter_unset = previous_fields.iter().cloned().collect();

            let affected_fields = if previous_fields.is_empty() {
                vec!["__schema__".to_string()]
            } else {
                previous_fields
            };
            for field in affected_fields {
                let diagnostic = ComputedFieldDiagnostic {
                    module: FORMULA_MODULE_ID.to_string(),
                    field: field.clone(),
                    code: "invalid_schema".to_string(),
                    message: message.to_string(),
                    span_start: None,
                    span_end: None,
                };
                entries.insert(
                    field.clone(),
                    ComputedFieldEntry {
                        module: FORMULA_MODULE_ID.to_string(),
                        definition_fingerprint: String::new(),
                        value_json: None,
                        diagnostic: Some(diagnostic),
                    },
                );
                execution.diagnostics.push(ModuleDiagnostic {
                    path: Some(path.clone()),
                    module: FORMULA_MODULE_ID.to_string(),
                    field,
                    code: "invalid_schema".to_string(),
                    message: message.to_string(),
                    span_start: None,
                    span_end: None,
                });
                execution.fields_updated += 1;
            }
            execution.derived_field_patches.push(DerivedFieldPatch {
                path: path.clone(),
                expected_content_hash: file.content_hash.clone(),
                fields: entries,
                frontmatter_set: BTreeMap::new(),
                frontmatter_unset,
            });
        }
    }

    fn persist_missing_overlay(
        &self,
        context: &ModuleContext<'_>,
        paths: &[String],
        execution: &mut ModuleExecution,
    ) {
        for path in paths {
            let Some(file) = context.files.get(path) else {
                continue;
            };
            execution.files_evaluated += 1;
            let mut entries = file.computed_fields.clone();
            let previous_fields: Vec<String> = entries
                .iter()
                .filter(|(_, entry)| entry.module == FORMULA_MODULE_ID)
                .map(|(field, _)| field.clone())
                .collect();
            entries.retain(|_, entry| entry.module != FORMULA_MODULE_ID);
            let frontmatter_unset = previous_fields.iter().cloned().collect();

            for field in previous_fields {
                let message =
                    "the schema overlay was removed; the cached formula value was cleared";
                entries.insert(
                    field.clone(),
                    ComputedFieldEntry {
                        module: FORMULA_MODULE_ID.to_string(),
                        definition_fingerprint: String::new(),
                        value_json: None,
                        diagnostic: Some(ComputedFieldDiagnostic {
                            module: FORMULA_MODULE_ID.to_string(),
                            field: field.clone(),
                            code: "schema_overlay_missing".to_string(),
                            message: message.to_string(),
                            span_start: None,
                            span_end: None,
                        }),
                    },
                );
                execution.diagnostics.push(ModuleDiagnostic {
                    path: Some(path.clone()),
                    module: FORMULA_MODULE_ID.to_string(),
                    field,
                    code: "schema_overlay_missing".to_string(),
                    message: message.to_string(),
                    span_start: None,
                    span_end: None,
                });
                execution.fields_updated += 1;
            }
            execution.derived_field_patches.push(DerivedFieldPatch {
                path: path.clone(),
                expected_content_hash: file.content_hash.clone(),
                fields: entries,
                frontmatter_set: BTreeMap::new(),
                frontmatter_unset,
            });
        }
    }

    fn schema_definitions(
        context: &ModuleContext<'_>,
        overlay: Option<&OverlaySchema>,
    ) -> ModuleSchemaPatch {
        // Preserve the ingest/watcher's raw schema snapshot. A module owns only
        // its formula definitions and result statistics; it must not make an
        // incremental single-file ingest look like a full raw-schema rebuild.
        let mut base = context.schema.cloned().unwrap_or_else(|| {
            Schema::infer_from_frontmatter_iter(std::iter::empty::<&JsonValue>())
        });
        let globally_owned: BTreeSet<&str> = context
            .files
            .values()
            .flat_map(|file| file.computed_fields.iter())
            .filter(|(_, entry)| entry.module == FORMULA_MODULE_ID)
            .map(|(field, _)| field.as_str())
            .collect();
        base.fields.retain(|field| {
            field.field_type != FieldType::Formula && !globally_owned.contains(field.name.as_str())
        });
        let overlay_fields = overlay.map(|overlay| overlay.fields.clone());
        let schema = Some(Schema::merge(base, overlay_fields));

        let mut scope_names = BTreeSet::new();
        for path in context.files.keys() {
            if let Some((top, _)) = path.split_once('/') {
                scope_names.insert(top.to_string());
            }
        }
        if let Some(existing) = context.scoped_schemas {
            for scoped in existing {
                if context
                    .files
                    .keys()
                    .any(|path| Self::path_matches_scope(path, &scoped.scope))
                {
                    scope_names.insert(scoped.scope.clone());
                }
            }
        }
        if let Some(overlay) = overlay {
            for scope in overlay.scopes.keys() {
                scope_names.insert(scope.clone());
            }
        }
        let mut existing_by_scope: BTreeMap<String, Schema> = context
            .scoped_schemas
            .unwrap_or_default()
            .iter()
            .map(|scoped| (scoped.scope.clone(), scoped.schema.clone()))
            .collect();

        let mut scoped_schemas = Vec::with_capacity(scope_names.len());
        for scope in scope_names {
            let mut raw_schema = existing_by_scope.remove(&scope).unwrap_or_else(|| {
                Schema::infer_from_frontmatter_iter(std::iter::empty::<&JsonValue>())
            });
            let scope_owned: BTreeSet<&str> = context
                .files
                .iter()
                .filter(|(path, _)| Self::path_matches_scope(path, &scope))
                .flat_map(|(_, file)| file.computed_fields.iter())
                .filter(|(_, entry)| entry.module == FORMULA_MODULE_ID)
                .map(|(field, _)| field.as_str())
                .collect();
            raw_schema.fields.retain(|field| {
                field.field_type != FieldType::Formula && !scope_owned.contains(field.name.as_str())
            });
            let overlay_fields =
                overlay.map(|overlay| Schema::resolve_overlay_for_path(overlay, Some(&scope)));
            scoped_schemas.push(crate::schema::ScopedSchema {
                scope,
                schema: Schema::merge(raw_schema, overlay_fields),
            });
        }
        ModuleSchemaPatch {
            schema,
            scoped_schemas: (!scoped_schemas.is_empty()).then_some(scoped_schemas),
        }
    }

    fn refresh_one_schema<'a>(
        schema: &mut Schema,
        files: impl IntoIterator<Item = &'a StoredFile>,
    ) {
        let files = files.into_iter().collect::<Vec<_>>();
        for field in &mut schema.fields {
            if field.field_type != FieldType::Formula {
                continue;
            }

            let mut count = 0usize;
            let mut samples = BTreeSet::new();
            for file in &files {
                let Some(entry) = file.computed_fields.get(&field.name) else {
                    continue;
                };
                if entry.module != FORMULA_MODULE_ID || entry.diagnostic.is_some() {
                    continue;
                }
                let Some(value) = entry
                    .value_json
                    .as_deref()
                    .and_then(|raw| serde_json::from_str::<JsonValue>(raw).ok())
                else {
                    continue;
                };
                if value.is_null() {
                    continue;
                }

                count += 1;
                if samples.len() < 20 {
                    samples.insert(match value {
                        JsonValue::String(value) => value,
                        value => value.to_string(),
                    });
                }
            }
            field.occurrence_count = count;
            field.sample_values = samples.into_iter().collect();
        }
    }

    fn finish_schema_patch(
        context: &ModuleContext<'_>,
        overlay: Option<&OverlaySchema>,
        execution: &mut ModuleExecution,
    ) {
        let mut files = context.files.clone();
        for patch in &execution.derived_field_patches {
            if let Some(file) = files.get_mut(&patch.path) {
                file.computed_fields = patch.fields.clone();
            }
        }

        let mut schema_patch = Self::schema_definitions(context, overlay);
        if let Some(schema) = &mut schema_patch.schema {
            Self::refresh_one_schema(schema, files.values());
        }
        if let Some(scoped_schemas) = &mut schema_patch.scoped_schemas {
            for scoped in scoped_schemas {
                let scope = scoped.scope.trim_matches('/');
                Self::refresh_one_schema(
                    &mut scoped.schema,
                    files.iter().filter_map(|(path, file)| {
                        Self::path_matches_scope(path, scope).then_some(file)
                    }),
                );
            }
        }
        execution.schema_patch = Some(schema_patch);
    }
}

impl Module for FormulaModule {
    fn descriptor(&self) -> ModuleDescriptor {
        ModuleDescriptor {
            id: FORMULA_MODULE_ID.to_string(),
            name: "Formula".to_string(),
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
        let mut paths = event_paths(event, context.files);
        let mut execution = ModuleExecution::default();
        let overlay = match Schema::load_overlay(context.project_root) {
            Ok(overlay) => overlay,
            Err(error) => {
                if matches!(event, ModuleEvent::FilesChanged { .. }) {
                    paths.extend(
                        context
                            .files
                            .iter()
                            .filter_map(|(path, file)| {
                                Self::has_formula_state(file).then_some(path)
                            })
                            .cloned(),
                    );
                    paths.sort();
                    paths.dedup();
                }
                self.persist_schema_error(context, &paths, &error.to_string(), &mut execution);
                Self::finish_schema_patch(context, None, &mut execution);
                return Ok(execution);
            }
        };
        if overlay.is_none() {
            if matches!(event, ModuleEvent::FilesChanged { .. }) {
                paths.extend(
                    context
                        .files
                        .iter()
                        .filter_map(|(path, file)| Self::has_formula_state(file).then_some(path))
                        .cloned(),
                );
                paths.sort();
                paths.dedup();
            }
            self.persist_missing_overlay(context, &paths, &mut execution);
            Self::finish_schema_patch(context, None, &mut execution);
            return Ok(execution);
        }
        if matches!(event, ModuleEvent::SchemaChanged) {
            if let Some(overlay) = overlay.as_ref() {
                paths.retain(|path| {
                    context
                        .files
                        .get(path)
                        .is_some_and(|file| self.needs_recompute(overlay, path, file))
                });
            }
        } else if matches!(event, ModuleEvent::FilesChanged { .. }) {
            if let Some(overlay) = overlay.as_ref() {
                paths.extend(context.files.iter().filter_map(|(path, file)| {
                    self.needs_recompute(overlay, path, file)
                        .then_some(path.clone())
                }));
                paths.sort();
                paths.dedup();
            }
        }

        // One scope frequently covers many files. Reuse its compiled graph by
        // definition fingerprint rather than parsing the same expressions for
        // every row.
        let mut programs = BTreeMap::<String, FormulaProgram>::new();
        for path in paths {
            let Some(file) = context.files.get(&path) else {
                continue;
            };
            execution.files_evaluated += 1;
            let definitions = overlay
                .as_ref()
                .map(|overlay| Self::definitions_for_path(overlay, &path))
                .unwrap_or_default();

            if definitions.is_empty() {
                let mut entries = file.computed_fields.clone();
                let before = entries.len();
                entries.retain(|_, entry| entry.module != FORMULA_MODULE_ID);
                execution.fields_updated += before - entries.len();
                execution.derived_field_patches.push(DerivedFieldPatch {
                    path,
                    expected_content_hash: file.content_hash.clone(),
                    fields: entries,
                    frontmatter_set: BTreeMap::new(),
                    frontmatter_unset: file
                        .computed_fields
                        .iter()
                        .filter(|(_, entry)| entry.module == FORMULA_MODULE_ID)
                        .map(|(field, _)| field.clone())
                        .collect(),
                });
                continue;
            }

            let program = self.engine.compile(definitions);
            let fingerprint = program.fingerprint().to_string();
            programs.entry(fingerprint.clone()).or_insert(program);
            let program = programs
                .get(&fingerprint)
                .expect("formula program was just inserted");
            self.evaluate_file(&path, file, program, &mut execution);
        }
        Self::finish_schema_patch(context, overlay.as_ref(), &mut execution);
        Ok(execution)
    }
}

/// Normalize a user-provided module scope to the index's slash-prefix form.
pub fn normalize_module_scope(scope: Option<&str>) -> Option<String> {
    let raw = scope?.trim().trim_start_matches("./").trim_matches('/');
    if raw.is_empty() || raw == "." {
        None
    } else {
        Some(format!("{raw}/"))
    }
}

/// Select paths affected by an event. Schema/full events intentionally select
/// the entire index because their definition diff is owned by the module.
pub(crate) fn event_paths(event: &ModuleEvent, files: &HashMap<String, StoredFile>) -> Vec<String> {
    let mut paths: Vec<String> = match event {
        ModuleEvent::FilesChanged { upserted, .. } => upserted.clone(),
        ModuleEvent::ManualRun { scope } => {
            let prefix = normalize_module_scope(scope.as_deref());
            files
                .keys()
                .filter(|path| {
                    prefix
                        .as_ref()
                        .is_none_or(|scope| crate::path_util::path_is_in_scope(path, scope))
                })
                .cloned()
                .collect()
        }
        ModuleEvent::FullIngest | ModuleEvent::SchemaChanged => files.keys().cloned().collect(),
    };
    paths.sort();
    paths.dedup();
    paths
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::Error;
    use crate::index::types::EmbeddingConfig;
    use tempfile::TempDir;

    #[test]
    fn normalize_scope_is_segment_prefix() {
        assert_eq!(normalize_module_scope(None), None);
        assert_eq!(normalize_module_scope(Some(".")), None);
        assert_eq!(
            normalize_module_scope(Some("./invoices/")),
            Some("invoices/".to_string())
        );
    }

    #[test]
    fn manual_event_paths_are_limited_to_the_requested_scope() {
        let dir = TempDir::new().unwrap();
        let index = Index::create(
            &dir.path().join("index.bin"),
            &EmbeddingConfig {
                provider: "test".to_string(),
                model: "test".to_string(),
                dimensions: 2,
            },
        )
        .unwrap();
        for path in ["invoices/a.md", "invoices-old/b.md", "notes/c.md"] {
            index.insert_file_hash_for_test(path, "hash");
        }

        assert_eq!(
            event_paths(
                &ModuleEvent::ManualRun {
                    scope: Some("invoices".to_string()),
                },
                &index.get_all_files(),
            ),
            vec!["invoices/a.md".to_string()]
        );
    }

    #[test]
    fn schema_change_evaluates_only_scopes_with_changed_fingerprints() {
        let dir = TempDir::new().unwrap();
        let index = Index::create(
            &dir.path().join("index.bin"),
            &EmbeddingConfig {
                provider: "test".to_string(),
                model: "test".to_string(),
                dimensions: 2,
            },
        )
        .unwrap();
        for (path, field, value) in [
            ("invoices/a.md", "total", "1"),
            ("notes/b.md", "score", "2"),
        ] {
            let absolute = dir.path().join(path);
            std::fs::create_dir_all(absolute.parent().unwrap()).unwrap();
            std::fs::write(&absolute, format!("---\n{field}: {value}\n---\nBody\n")).unwrap();
            let parsed = crate::parser::parse_markdown_file(dir.path(), Path::new(path)).unwrap();
            index.upsert(&parsed, &[], &[]).unwrap();
        }

        let engine = FormulaEngine::default();
        let definitions = [
            ("invoices/a.md", "total", "1"),
            ("notes/b.md", "score", "2"),
        ];
        for (path, field, formula) in definitions {
            let fingerprint = engine
                .compile(vec![FormulaDefinition::new(
                    field,
                    formula,
                    FormulaResultType::Number,
                )])
                .fingerprint()
                .to_string();
            index
                .replace_computed_fields(
                    path,
                    HashMap::from([(
                        field.to_string(),
                        ComputedFieldEntry {
                            module: FORMULA_MODULE_ID.to_string(),
                            definition_fingerprint: fingerprint,
                            value_json: Some(formula.to_string()),
                            diagnostic: None,
                        },
                    )]),
                )
                .unwrap();
        }

        std::fs::write(
            dir.path().join(".markdownvdb.schema.yml"),
            "scopes:\n  invoices:\n    fields:\n      total:\n        field_type: formula\n        formula: '3'\n        result_type: number\n  notes:\n    fields:\n      score:\n        field_type: formula\n        formula: '2'\n        result_type: number\n",
        )
        .unwrap();

        let report = ModuleRunner::builtins()
            .run_one(
                FORMULA_MODULE_ID,
                dir.path(),
                &index,
                &ModuleEvent::SchemaChanged,
            )
            .unwrap();
        assert_eq!(report.files_evaluated, 1);
        assert_eq!(
            index.get_computed_fields("invoices/a.md").unwrap()["total"]
                .value_json
                .as_deref(),
            Some("3")
        );
        assert_eq!(
            index.get_computed_fields("notes/b.md").unwrap()["score"]
                .value_json
                .as_deref(),
            Some("2")
        );
    }

    struct FailureModule;

    impl Module for FailureModule {
        fn descriptor(&self) -> ModuleDescriptor {
            ModuleDescriptor {
                id: "failing".to_string(),
                name: "Failing test module".to_string(),
                version: 1,
                always_on: true,
                hooks: vec!["manual_run".to_string()],
            }
        }

        fn run(
            &self,
            _context: &ModuleContext<'_>,
            _event: &ModuleEvent,
        ) -> crate::Result<ModuleExecution> {
            Err(Error::Config("intentional module failure".to_string()))
        }
    }

    #[test]
    fn failed_module_clears_its_values_and_preserves_other_modules() {
        let dir = TempDir::new().unwrap();
        let index = Index::create(
            &dir.path().join("index.bin"),
            &EmbeddingConfig {
                provider: "test".to_string(),
                model: "test".to_string(),
                dimensions: 2,
            },
        )
        .unwrap();
        index.insert_file_hash_for_test("invoice.md", "hash");

        let mut original = HashMap::new();
        original.insert(
            "total".to_string(),
            ComputedFieldEntry {
                module: "failing".to_string(),
                definition_fingerprint: "old".to_string(),
                value_json: Some("12.3".to_string()),
                diagnostic: None,
            },
        );
        original.insert(
            "other".to_string(),
            ComputedFieldEntry {
                module: "other".to_string(),
                definition_fingerprint: "stable".to_string(),
                value_json: Some("\"kept\"".to_string()),
                diagnostic: None,
            },
        );
        index
            .replace_computed_fields("invoice.md", original)
            .unwrap();

        let runner = ModuleRunner::new(vec![Box::new(FailureModule)]);
        let report = runner
            .run_one(
                "failing",
                dir.path(),
                &index,
                &ModuleEvent::ManualRun { scope: None },
            )
            .unwrap();

        assert_eq!(report.diagnostics[0].code, "module_error");
        let fields = index.get_file("invoice.md").unwrap().computed_fields;
        assert_eq!(
            fields["total"].diagnostic.as_ref().unwrap().code,
            "module_error"
        );
        assert!(fields["total"].value_json.is_none());
        assert_eq!(fields["other"].value_json.as_deref(), Some("\"kept\""));
    }

    struct FormulaFailureModule;

    impl Module for FormulaFailureModule {
        fn descriptor(&self) -> ModuleDescriptor {
            ModuleDescriptor {
                id: FORMULA_MODULE_ID.to_string(),
                name: "Formula failure test".to_string(),
                version: 1,
                always_on: true,
                hooks: vec!["manual_run".to_string()],
            }
        }

        fn run(
            &self,
            _context: &ModuleContext<'_>,
            _event: &ModuleEvent,
        ) -> crate::Result<ModuleExecution> {
            Err(Error::Config("intentional formula failure".to_string()))
        }
    }

    #[test]
    fn top_level_formula_failure_removes_materialized_source_value() {
        let dir = TempDir::new().unwrap();
        std::fs::write(
            dir.path().join("invoice.md"),
            "---\nprice: 2\ntotal: 4\n---\nBody\n",
        )
        .unwrap();
        let index = Index::create(
            &dir.path().join("index.bin"),
            &EmbeddingConfig {
                provider: "test".to_string(),
                model: "test".to_string(),
                dimensions: 2,
            },
        )
        .unwrap();
        let parsed =
            crate::parser::parse_markdown_file(dir.path(), Path::new("invoice.md")).unwrap();
        index.upsert(&parsed, &[], &[]).unwrap();
        index
            .replace_computed_fields(
                "invoice.md",
                HashMap::from([(
                    "total".to_string(),
                    ComputedFieldEntry {
                        module: FORMULA_MODULE_ID.to_string(),
                        definition_fingerprint: "old".to_string(),
                        value_json: Some("4".to_string()),
                        diagnostic: None,
                    },
                )]),
            )
            .unwrap();

        let report = ModuleRunner::new(vec![Box::new(FormulaFailureModule)])
            .run_one(
                FORMULA_MODULE_ID,
                dir.path(),
                &index,
                &ModuleEvent::ManualRun { scope: None },
            )
            .unwrap();
        assert_eq!(report.diagnostics[0].code, "module_error");
        let rewritten = std::fs::read_to_string(dir.path().join("invoice.md")).unwrap();
        assert!(!rewritten.contains("total:"));
        let stored = index.get_file("invoice.md").unwrap();
        assert_eq!(
            stored.computed_fields["total"]
                .diagnostic
                .as_ref()
                .unwrap()
                .code,
            "module_error"
        );
        assert!(stored.computed_fields["total"].value_json.is_none());
    }
}
