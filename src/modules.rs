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
use sha2::{Digest, Sha256};

use crate::formula::{
    FormulaDefinition, FormulaDiagnostic, FormulaEngine, FormulaProgram, FORMULA_MODULE_ID,
};
use crate::index::state::Index;
use crate::index::types::{
    ComputedDependencySnapshot, ComputedFieldDiagnostic, ComputedFieldEntry, StoredFile,
};
use crate::schema::{FieldType, FormulaResultType, OverlaySchema, Schema};

#[path = "lookup_rollup.rs"]
mod lookup_rollup;

pub use lookup_rollup::{LookupRollupModule, LOOKUP_ROLLUP_MODULE_ID};

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
    /// Internal manual-run lens after dependency planning has resolved an
    /// exact set of output-owner documents.
    ManualPaths {
        paths: Vec<String>,
    },
}

impl ModuleEvent {
    pub fn name(&self) -> &'static str {
        match self {
            Self::FullIngest => "full_ingest",
            Self::FilesChanged { .. } => "files_changed",
            Self::SchemaChanged => "schema_changed",
            Self::ManualRun { .. } | Self::ManualPaths { .. } => "manual_run",
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

/// One computed-module diagnostic with optional source and document location.
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
    /// Exact schema-overlay byte snapshot used to compile definitions.
    pub expected_schema_fingerprint: Option<String>,
    /// Filesystem content hashes read while evaluating cross-document inputs.
    /// The runner verifies this coherent dependency snapshot once immediately
    /// before applying any owner patches.
    pub expected_dependency_hashes: BTreeMap<String, String>,
    /// Resolved targets that did not exist in the evaluation snapshot.
    pub expected_missing_dependency_states: BTreeMap<String, bool>,
    /// Indexed collection membership for every incoming Rollup scope.
    pub expected_incoming_scope_membership: BTreeMap<String, BTreeSet<String>>,
    /// Output fields whose prior materialization must be suppressed if the
    /// coherent dependency snapshot fails before any patch is necessary.
    pub dependency_owners: BTreeMap<String, BTreeSet<String>>,
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

/// Aggregate report for a requested manual module run.
///
/// The requested module remains flattened at the top level for backward-
/// compatible CLI JSON, while `module_reports` exposes the complete ordered
/// built-in sequence needed to satisfy inter-module dependencies.
#[derive(Debug, Clone, Serialize)]
pub struct ModuleRunReport {
    #[serde(flatten)]
    pub requested: ModuleReport,
    pub module_reports: Vec<ModuleReport>,
}

/// Restricted host context supplied to a compiled-in module.
pub struct ModuleContext<'a> {
    pub project_root: &'a Path,
    pub files: &'a HashMap<String, StoredFile>,
    pub schema: Option<&'a Schema>,
    pub scoped_schemas: Option<&'a [crate::schema::ScopedSchema]>,
}

fn refresh_computed_schema_stats<'a>(
    schema: &mut Schema,
    files: impl IntoIterator<Item = &'a StoredFile>,
) {
    let files: Vec<&StoredFile> = files.into_iter().collect();
    for field in &mut schema.fields {
        let module = match field.field_type {
            FieldType::Formula => FORMULA_MODULE_ID,
            FieldType::Lookup | FieldType::Rollup => LOOKUP_ROLLUP_MODULE_ID,
            _ => continue,
        };
        let mut count = 0usize;
        let mut samples = BTreeSet::new();
        for file in &files {
            let Some(entry) = file.computed_fields.get(&field.name) else {
                continue;
            };
            if entry.module != module
                || entry.diagnostic.is_some()
                || !file.materialized_field_matches(&field.name, entry)
            {
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
                    other => other.to_string(),
                });
            }
        }
        field.occurrence_count = count;
        field.sample_values = samples.into_iter().collect();
    }
}

fn computed_schema_candidate_names(
    context: &ModuleContext<'_>,
    overlay: Option<&OverlaySchema>,
) -> BTreeSet<String> {
    let mut names = BTreeSet::new();
    if let Some(schema) = context.schema {
        names.extend(
            schema
                .fields
                .iter()
                .filter(|field| field.field_type.is_computed())
                .map(|field| field.name.clone()),
        );
    }
    if let Some(scoped_schemas) = context.scoped_schemas {
        names.extend(scoped_schemas.iter().flat_map(|scoped| {
            scoped
                .schema
                .fields
                .iter()
                .filter(|field| field.field_type.is_computed())
                .map(|field| field.name.clone())
        }));
    }
    names.extend(context.files.values().flat_map(|file| {
        file.computed_fields
            .iter()
            .filter(|(field, entry)| file.materialized_field_matches(field, entry))
            .map(|(field, _)| field.clone())
    }));
    if let Some(overlay) = overlay {
        names.extend(
            overlay
                .fields
                .iter()
                .chain(
                    overlay
                        .scopes
                        .values()
                        .flat_map(|scope| scope.fields.iter()),
                )
                .filter(|(_, definition)| {
                    definition
                        .field_type
                        .as_deref()
                        .and_then(crate::schema::parse_field_type_str)
                        .is_some_and(|field_type| field_type.is_computed())
                })
                .map(|(field, _)| field.clone()),
        );
    }
    names
}

/// Re-infer only names that may have been polluted by materialized computed
/// values. Per-path ownership/overlay resolution keeps an ordinary field with
/// the same name in another scope while excluding the computed occurrences.
fn ordinary_schema_for_computed_candidates(
    context: &ModuleContext<'_>,
    overlay: Option<&OverlaySchema>,
    candidates: &BTreeSet<String>,
    scope: Option<&str>,
) -> Schema {
    let mut frontmatters = Vec::new();
    for (path, file) in context.files {
        if scope.is_some_and(|scope| !crate::path_util::path_is_in_scope(path, scope)) {
            continue;
        }
        let Some(frontmatter) = file
            .frontmatter
            .as_deref()
            .and_then(|raw| serde_json::from_str::<JsonValue>(raw).ok())
            .and_then(|value| value.as_object().cloned())
        else {
            continue;
        };
        let resolved_overlay =
            overlay.map(|overlay| Schema::resolve_overlay_for_path(overlay, Some(path)));
        let mut ordinary = JsonMap::new();
        for candidate in candidates {
            let module_owned = file
                .computed_fields
                .get(candidate)
                .is_some_and(|entry| file.materialized_field_matches(candidate, entry));
            let schema_owned = resolved_overlay
                .as_ref()
                .and_then(|fields| fields.get(candidate))
                .and_then(|definition| definition.field_type.as_deref())
                .and_then(crate::schema::parse_field_type_str)
                .is_some_and(|field_type| field_type.is_computed());
            if !module_owned && !schema_owned {
                if let Some(value) = frontmatter.get(candidate) {
                    ordinary.insert(candidate.clone(), value.clone());
                }
            }
        }
        if !ordinary.is_empty() {
            frontmatters.push(JsonValue::Object(ordinary));
        }
    }
    Schema::infer_from_frontmatter_iter(frontmatters.iter())
}

fn replace_computed_candidates_with_ordinary(
    mut schema: Schema,
    ordinary: Schema,
    candidates: &BTreeSet<String>,
) -> Schema {
    schema
        .fields
        .retain(|field| !candidates.contains(&field.name));
    schema.fields.extend(ordinary.fields);
    schema
        .fields
        .sort_by(|left, right| left.name.cmp(&right.name));
    schema
}

/// Preserve the ingest/watcher's raw schema snapshot while excluding every
/// computed materialization, then reapply the live overlay. The final built-in
/// module uses this after cleanup so removed definitions cannot survive as
/// ghost columns without turning a single-file ingest into a full raw-schema
/// rebuild.
fn rebuild_schema_without_computed_materializations(
    context: &ModuleContext<'_>,
    overlay: Option<&OverlaySchema>,
) -> ModuleSchemaPatch {
    let candidates = computed_schema_candidate_names(context, overlay);
    let base_schema = context
        .schema
        .cloned()
        .unwrap_or_else(|| Schema::infer_from_frontmatter_iter(std::iter::empty::<&JsonValue>()));
    let ordinary = ordinary_schema_for_computed_candidates(context, overlay, &candidates, None);
    let raw_schema = replace_computed_candidates_with_ordinary(base_schema, ordinary, &candidates);
    let schema = Some(Schema::merge(
        raw_schema,
        overlay.map(|overlay| overlay.fields.clone()),
    ));

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
                .any(|path| crate::path_util::path_is_in_scope(path, &scoped.scope))
            {
                scope_names.insert(scoped.scope.clone());
            }
        }
    }
    if let Some(overlay) = overlay {
        scope_names.extend(overlay.scopes.keys().cloned());
    }

    let mut existing_by_scope: BTreeMap<String, Schema> = context
        .scoped_schemas
        .unwrap_or_default()
        .iter()
        .map(|scoped| (scoped.scope.clone(), scoped.schema.clone()))
        .collect();
    let mut scoped_schemas = Vec::with_capacity(scope_names.len());
    for scope in scope_names {
        let base_schema = existing_by_scope.remove(&scope).unwrap_or_else(|| {
            Schema::infer_from_frontmatter_iter(std::iter::empty::<&JsonValue>())
        });
        let ordinary =
            ordinary_schema_for_computed_candidates(context, overlay, &candidates, Some(&scope));
        let raw_schema =
            replace_computed_candidates_with_ordinary(base_schema, ordinary, &candidates);
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

/// A built-in module. Implementations receive a read-only state snapshot and
/// return patches; only [`ModuleRunner`] mutates the index.
pub trait Module: Send + Sync {
    fn descriptor(&self) -> ModuleDescriptor;

    fn run(
        &self,
        context: &ModuleContext<'_>,
        event: &ModuleEvent,
    ) -> crate::Result<ModuleExecution>;

    /// Refresh this module's computed occurrence/sample statistics after the
    /// runner has applied source patches (including per-file write failures).
    fn refresh_schema(
        &self,
        _schema: &mut Schema,
        _files: &HashMap<String, StoredFile>,
        _scope: Option<&str>,
    ) {
    }
}

/// Runs all registered modules in deterministic registration order.
pub struct ModuleRunner {
    modules: Vec<Box<dyn Module>>,
}

#[must_use]
pub struct ModuleRunLock {
    _file: std::fs::File,
    root_guard: crate::frontmatter_write::ProjectRootGuard,
}

impl ModuleRunLock {
    pub(crate) fn verify_project_root(&self, project_root: &Path) -> crate::Result<()> {
        self.root_guard.verify(project_root)
    }
}

/// Acquire the project-scoped computed-module lock.
///
/// This is public so trusted frontends can keep an overlay mutation and the
/// immediately following module pipeline inside one cross-process critical
/// section. Callers must not hold it across unrelated user interaction.
pub fn acquire_module_run_lock(project_root: &Path) -> crate::Result<ModuleRunLock> {
    use std::time::Duration;

    const ATTEMPTS: usize = 40;
    const RETRY_DELAY: Duration = Duration::from_millis(25);

    let lock_path = project_root.join(".markdownvdb/modules.lock");
    let (file, root_guard) = crate::frontmatter_write::open_module_run_lock_file(project_root)?;
    for attempt in 0..ATTEMPTS {
        match file.try_lock() {
            Ok(()) => {
                let lock = ModuleRunLock {
                    _file: file,
                    root_guard,
                };
                // Close the acquisition window between opening the stable root
                // descriptor and successfully taking its state-directory lock.
                lock.verify_project_root(project_root)?;
                return Ok(lock);
            }
            Err(std::fs::TryLockError::WouldBlock) if attempt + 1 < ATTEMPTS => {
                std::thread::sleep(RETRY_DELAY);
            }
            Err(std::fs::TryLockError::WouldBlock) => {
                return Err(crate::Error::Config(format!(
                    "another computed-module run holds `{}`",
                    lock_path.display()
                )));
            }
            Err(std::fs::TryLockError::Error(error)) => return Err(crate::Error::Io(error)),
        }
    }
    unreachable!("module lock retry loop always returns")
}

struct DerivedStateSnapshot {
    files: HashMap<String, StoredFile>,
    schema: Option<Schema>,
    scoped_schemas: Option<Vec<crate::schema::ScopedSchema>>,
}

/// Fields that this module demonstrably owns in the current source snapshot.
/// Provenance or key presence alone is insufficient: the current semantic
/// value must exactly match the module's last successful materialization.
fn materialized_fields_for_module(file: &StoredFile, module: &str) -> BTreeSet<String> {
    file.computed_fields
        .iter()
        .filter(|(field, entry)| {
            entry.module == module && file.materialized_field_matches(field, entry)
        })
        .map(|(field, _)| field.clone())
        .collect()
}

fn restore_materialized_proofs(
    fields: &mut HashMap<String, ComputedFieldEntry>,
    previous: &HashMap<String, ComputedFieldEntry>,
    materialized: &BTreeSet<String>,
) {
    for field in materialized {
        if let Some(entry) = fields.get_mut(field) {
            entry.materialized_value_json = previous
                .get(field)
                .and_then(|entry| entry.materialized_value_json.clone());
        }
    }
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
        lock: &ModuleRunLock,
        module: &str,
        message: &str,
        additional_owners: &BTreeMap<String, BTreeSet<String>>,
    ) {
        if lock.verify_project_root(project_root).is_err() {
            return;
        }
        index.set_schema(self.schema.clone());
        index.set_scoped_schemas(self.scoped_schemas.clone());
        for (path, file) in &self.files {
            let mut fields = file.computed_fields.clone();
            let materialized_fields = materialized_fields_for_module(file, module);
            let mut affected_fields = materialized_fields.clone();
            affected_fields.extend(
                additional_owners
                    .get(path)
                    .into_iter()
                    .flat_map(|fields| fields.iter().cloned()),
            );
            for field in &affected_fields {
                if fields
                    .get(field)
                    .is_some_and(|entry| entry.module != module)
                {
                    continue;
                }
                let entry = fields
                    .entry(field.clone())
                    .or_insert_with(|| ComputedFieldEntry {
                        module: module.to_string(),
                        definition_fingerprint: String::new(),
                        input_fingerprint: None,
                        dependency_snapshot: ComputedDependencySnapshot::default(),
                        value_json: None,
                        materialized_value_json: None,
                        diagnostic: None,
                    });
                entry.value_json = None;
                entry.materialized_value_json = None;
                entry.diagnostic = Some(ComputedFieldDiagnostic {
                    module: module.to_string(),
                    field: field.clone(),
                    code: "module_error".to_string(),
                    message: message.to_string(),
                    span_start: None,
                    span_end: None,
                });
            }

            if !materialized_fields.is_empty() {
                let root_guard = || lock.verify_project_root(project_root);
                if let Ok(writeback) =
                    crate::frontmatter_write::apply_frontmatter_patch_with_intent_and_guard(
                        project_root,
                        Path::new(path),
                        &file.content_hash,
                        &BTreeMap::new(),
                        &materialized_fields,
                        crate::frontmatter_write::ComputedWriteContext {
                            owned_unset: &materialized_fields,
                            fields: &fields,
                            pre_commit_guard: Some(&root_guard),
                        },
                    )
                {
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
            restore_materialized_proofs(&mut fields, &file.computed_fields, &materialized_fields);
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
        // Formula deliberately runs first: Lookup/Rollup fields may retrieve or
        // aggregate a freshly calculated Formula value from a related document.
        Self::new(vec![
            Box::new(FormulaModule::default()),
            Box::new(LookupRollupModule::default()),
        ])
    }

    pub fn descriptors(&self) -> Vec<ModuleDescriptor> {
        self.modules
            .iter()
            .map(|module| module.descriptor())
            .collect()
    }

    fn recover_pending_writebacks(
        project_root: &Path,
        index: &Index,
        lock: &ModuleRunLock,
    ) -> crate::Result<bool> {
        lock.verify_project_root(project_root)?;
        let recovered = crate::frontmatter_write::recover_computed_intents(project_root, index)?;
        lock.verify_project_root(project_root)?;
        Ok(recovered)
    }

    fn save_and_finish_writebacks(
        project_root: &Path,
        index: &Index,
        lock: &ModuleRunLock,
    ) -> crate::Result<()> {
        lock.verify_project_root(project_root)?;
        index.save()?;
        lock.verify_project_root(project_root)?;
        crate::frontmatter_write::finish_computed_intents(project_root, index)?;
        lock.verify_project_root(project_root)
    }

    fn dependency_change(
        project_root: &Path,
        execution: &ModuleExecution,
        lock: &ModuleRunLock,
    ) -> crate::Result<Option<(String, String, Option<String>)>> {
        lock.verify_project_root(project_root)?;
        let schema_change = execution
            .expected_schema_fingerprint
            .as_ref()
            .and_then(|expected| {
                let actual = Schema::overlay_source_fingerprint(project_root)
                    .unwrap_or_else(|error| format!("unreadable:{error}"));
                (actual != *expected).then(|| {
                    (
                        ".markdownvdb.schema.yml".to_string(),
                        expected.clone(),
                        Some(actual),
                    )
                })
            });
        let content_change = || {
            execution
                .expected_dependency_hashes
                .iter()
                .find_map(|(path, expected)| {
                    let actual = crate::parser::parse_markdown_file(project_root, Path::new(path))
                        .ok()
                        .map(|file| file.content_hash);
                    (actual.as_deref() != Some(expected.as_str()))
                        .then(|| (path.clone(), expected.clone(), actual))
                })
        };
        let missing_change = || {
            execution
                .expected_missing_dependency_states
                .iter()
                .find_map(|(path, expected_exists)| {
                    let actual_exists = project_root.join(path).is_file();
                    (actual_exists != *expected_exists).then(|| {
                        (
                            path.clone(),
                            format!("exists={expected_exists}"),
                            Some(format!("exists={actual_exists}")),
                        )
                    })
                })
        };
        let membership_change = || -> crate::Result<Option<(String, String, Option<String>)>> {
            if execution.expected_incoming_scope_membership.is_empty() {
                return Ok(None);
            }
            let config = crate::config::Config::load(project_root)?;
            let discovered: BTreeSet<String> =
                crate::discovery::FileDiscovery::new(project_root, &config)
                    .discover()?
                    .into_iter()
                    .map(|path| crate::path_util::to_slash(&path))
                    .collect();
            Ok(execution
                .expected_incoming_scope_membership
                .iter()
                .find_map(|(scope, expected)| {
                    let actual: BTreeSet<String> = discovered
                        .iter()
                        .filter(|path| crate::path_util::path_is_in_scope(path, scope))
                        .cloned()
                        .collect();
                    (&actual != expected).then(|| {
                        (
                            format!("incoming scope `{scope}`"),
                            format!("{:?}", expected),
                            Some(format!("{:?}", actual)),
                        )
                    })
                }))
        };
        if let Some(change) = schema_change
            .or_else(content_change)
            .or_else(missing_change)
        {
            lock.verify_project_root(project_root)?;
            return Ok(Some(change));
        }
        let change = membership_change()?;
        lock.verify_project_root(project_root)?;
        Ok(change)
    }

    fn suppress_dependency_change(
        module_id: &str,
        project_root: &Path,
        index: &Index,
        lock: &ModuleRunLock,
        execution: &mut ModuleExecution,
        path: String,
    ) -> crate::Result<()> {
        let message = format!(
            "dependency `{path}` changed after evaluation; affected computed values were suppressed for retry"
        );
        let proposed: BTreeMap<String, HashMap<String, ComputedFieldEntry>> = execution
            .derived_field_patches
            .iter()
            .map(|patch| (patch.path.clone(), patch.fields.clone()))
            .collect();
        let mut owners = execution.dependency_owners.clone();
        for patch in &execution.derived_field_patches {
            owners.entry(patch.path.clone()).or_default().extend(
                patch
                    .frontmatter_unset
                    .iter()
                    .chain(patch.frontmatter_set.keys())
                    .cloned(),
            );
        }
        for (owner_path, affected) in owners {
            if affected.is_empty() {
                continue;
            }
            let mut fields = index.get_computed_fields(&owner_path).unwrap_or_default();
            let previous_fields = fields.clone();
            let stored = index.get_file(&owner_path);
            let materialized: BTreeSet<String> = stored
                .as_ref()
                .map(|stored| {
                    materialized_fields_for_module(stored, module_id)
                        .intersection(&affected)
                        .cloned()
                        .collect()
                })
                .unwrap_or_default();
            for field in &affected {
                let candidate = proposed
                    .get(&owner_path)
                    .and_then(|entries| entries.get(field))
                    .or_else(|| fields.get(field));
                let definition_fingerprint = candidate
                    .map(|entry| entry.definition_fingerprint.clone())
                    .unwrap_or_default();
                fields.insert(
                    field.clone(),
                    ComputedFieldEntry {
                        module: module_id.to_string(),
                        definition_fingerprint,
                        input_fingerprint: None,
                        dependency_snapshot: ComputedDependencySnapshot::default(),
                        value_json: None,
                        materialized_value_json: None,
                        diagnostic: Some(ComputedFieldDiagnostic {
                            module: module_id.to_string(),
                            field: field.clone(),
                            code: "dependency_changed".to_string(),
                            message: message.clone(),
                            span_start: None,
                            span_end: None,
                        }),
                    },
                );
                execution.diagnostics.push(ModuleDiagnostic {
                    path: Some(owner_path.clone()),
                    module: module_id.to_string(),
                    field: field.clone(),
                    code: "dependency_changed".to_string(),
                    message: message.clone(),
                    span_start: None,
                    span_end: None,
                });
            }
            if let Some(stored) = stored {
                if !materialized.is_empty() {
                    let root_guard = || lock.verify_project_root(project_root);
                    match crate::frontmatter_write::apply_frontmatter_patch_with_intent_and_guard(
                        project_root,
                        Path::new(&owner_path),
                        &stored.content_hash,
                        &BTreeMap::new(),
                        &materialized,
                        crate::frontmatter_write::ComputedWriteContext {
                            owned_unset: &materialized,
                            fields: &fields,
                            pre_commit_guard: Some(&root_guard),
                        },
                    ) {
                        Ok(writeback) => {
                            index.apply_module_source_state(
                                &stored.content_hash,
                                &writeback.file,
                                fields,
                            )?;
                            continue;
                        }
                        Err(_) => {
                            // The owner changed or cannot be safely patched. Keep
                            // its Markdown bytes untouched and persist only the
                            // diagnostic suppression for the next retry.
                        }
                    }
                }
            }
            restore_materialized_proofs(&mut fields, &previous_fields, &materialized);
            index.replace_computed_fields(&owner_path, fields)?;
        }
        execution.fields_updated = 0;
        execution.derived_field_patches.clear();
        execution.schema_patch = None;
        Ok(())
    }

    fn apply_execution(
        module: &dyn Module,
        project_root: &Path,
        index: &Index,
        lock: &ModuleRunLock,
        execution: &mut ModuleExecution,
    ) -> crate::Result<()> {
        lock.verify_project_root(project_root)?;
        let module_id = module.descriptor().id;
        if let Some((path, _, _)) = Self::dependency_change(project_root, execution, lock)? {
            Self::suppress_dependency_change(
                &module_id,
                project_root,
                index,
                lock,
                execution,
                path,
            )?;
        }
        let mut patch_index = 0;
        while patch_index < execution.derived_field_patches.len() {
            if patch_index > 0 {
                if let Some((path, _, _)) = Self::dependency_change(project_root, execution, lock)?
                {
                    Self::suppress_dependency_change(
                        &module_id,
                        project_root,
                        index,
                        lock,
                        execution,
                        path,
                    )?;
                    break;
                }
            }
            let dependency_guard_execution = execution.clone();
            let patch = &mut execution.derived_field_patches[patch_index];
            let relative_path = Path::new(&patch.path);
            let materialized_fields = index
                .get_file(&patch.path)
                .map(|file| materialized_fields_for_module(&file, &module_id))
                .unwrap_or_default();
            let previous_fields = index.get_computed_fields(&patch.path).unwrap_or_default();
            let dependency_guard = || {
                if let Some((dependency, _, _)) =
                    Self::dependency_change(project_root, &dependency_guard_execution, lock)?
                {
                    Err(crate::Error::DependencyChanged { dependency })
                } else {
                    Ok(())
                }
            };
            match crate::frontmatter_write::apply_frontmatter_patch_with_intent_and_guard(
                project_root,
                relative_path,
                &patch.expected_content_hash,
                &patch.frontmatter_set,
                &patch.frontmatter_unset,
                crate::frontmatter_write::ComputedWriteContext {
                    owned_unset: &materialized_fields,
                    fields: &patch.fields,
                    pre_commit_guard: Some(&dependency_guard),
                },
            ) {
                Ok(writeback) => {
                    let mut committed_fields = patch.fields.clone();
                    crate::frontmatter_write::normalize_committed_ownership(
                        &mut committed_fields,
                        &patch.frontmatter_set,
                        &patch.frontmatter_unset,
                        &writeback.materialized_fields,
                    )?;
                    index.apply_module_source_state(
                        &patch.expected_content_hash,
                        &writeback.file,
                        committed_fields.clone(),
                    )?;
                    patch.fields = committed_fields;
                    execution
                        .expected_dependency_hashes
                        .insert(patch.path.clone(), writeback.file.content_hash.clone());
                }
                Err(error) => {
                    let code = match &error {
                        crate::Error::SourceChanged { .. } => "source_changed",
                        crate::Error::DependencyChanged { .. } => "dependency_changed",
                        _ => "writeback_failed",
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
                            module: module_id.clone(),
                            field: field.clone(),
                            code: code.to_string(),
                            message: message.clone(),
                            span_start: None,
                            span_end: None,
                        };
                        patch.fields.insert(
                            field.clone(),
                            ComputedFieldEntry {
                                module: module_id.clone(),
                                definition_fingerprint,
                                input_fingerprint: None,
                                dependency_snapshot: ComputedDependencySnapshot::default(),
                                value_json: None,
                                materialized_value_json: previous_fields
                                    .get(&field)
                                    .filter(|_| materialized_fields.contains(&field))
                                    .and_then(|entry| entry.materialized_value_json.clone()),
                                diagnostic: Some(diagnostic),
                            },
                        );
                        execution.diagnostics.push(ModuleDiagnostic {
                            path: Some(patch.path.clone()),
                            module: module_id.clone(),
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
            patch_index += 1;
        }
        if !execution.derived_field_patches.is_empty() {
            if let Some((path, _, _)) = Self::dependency_change(project_root, execution, lock)? {
                Self::suppress_dependency_change(
                    &module_id,
                    project_root,
                    index,
                    lock,
                    execution,
                    path,
                )?;
            }
        }

        if let Some(patch) = &mut execution.schema_patch {
            lock.verify_project_root(project_root)?;
            let files = index.get_all_files();
            if let Some(schema) = &mut patch.schema {
                module.refresh_schema(schema, &files, None);
            }
            if let Some(scoped_schemas) = &mut patch.scoped_schemas {
                for scoped in scoped_schemas {
                    module.refresh_schema(&mut scoped.schema, &files, Some(&scoped.scope));
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

    fn lock_failure_report(
        module: &dyn Module,
        event: &ModuleEvent,
        message: String,
    ) -> ModuleReport {
        let descriptor = module.descriptor();
        ModuleReport {
            module: descriptor.id.clone(),
            event: event.name().to_string(),
            files_evaluated: 0,
            fields_updated: 0,
            diagnostics: Self::failure_execution(&descriptor.id, message).diagnostics,
            duration_ms: 0,
        }
    }

    fn execute(
        module: &dyn Module,
        project_root: &Path,
        index: &Index,
        event: &ModuleEvent,
        lock: &ModuleRunLock,
    ) -> ModuleReport {
        let descriptor = module.descriptor();
        let started = Instant::now();
        if let Err(error) = lock.verify_project_root(project_root) {
            let execution = Self::failure_execution(&descriptor.id, error.to_string());
            return ModuleReport {
                module: descriptor.id,
                event: event.name().to_string(),
                files_evaluated: 0,
                fields_updated: 0,
                diagnostics: execution.diagnostics,
                duration_ms: started.elapsed().as_millis() as u64,
            };
        }
        let snapshot = DerivedStateSnapshot::capture(index);
        let context = snapshot.context(project_root);
        let execution = match module.run(&context, event) {
            Ok(mut execution) => {
                let mut additional_owners = execution.dependency_owners.clone();
                for patch in &execution.derived_field_patches {
                    additional_owners
                        .entry(patch.path.clone())
                        .or_default()
                        .extend(
                            patch
                                .frontmatter_unset
                                .iter()
                                .chain(patch.frontmatter_set.keys())
                                .cloned(),
                        );
                }
                match Self::apply_execution(module, project_root, index, lock, &mut execution) {
                    Ok(()) => execution,
                    Err(error) => {
                        let message = error.to_string();
                        snapshot.restore_failed_module(
                            project_root,
                            index,
                            lock,
                            &descriptor.id,
                            &message,
                            &additional_owners,
                        );
                        Self::failure_execution(&descriptor.id, message)
                    }
                }
            }
            Err(error) => {
                let message = error.to_string();
                snapshot.restore_failed_module(
                    project_root,
                    index,
                    lock,
                    &descriptor.id,
                    &message,
                    &BTreeMap::new(),
                );
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

    fn schema_matches(
        project_root: &Path,
        expected: &str,
        lock: &ModuleRunLock,
    ) -> crate::Result<bool> {
        lock.verify_project_root(project_root)?;
        let matches = Schema::overlay_source_fingerprint(project_root)? == expected;
        lock.verify_project_root(project_root)?;
        Ok(matches)
    }

    fn suppress_for_unstable_schema(
        project_root: &Path,
        index: &Index,
        lock: &ModuleRunLock,
        message: &str,
    ) -> crate::Result<()> {
        lock.verify_project_root(project_root)?;
        for (path, file) in index.get_all_files() {
            let mut fields = file.computed_fields.clone();
            let mut affected = BTreeSet::new();
            for (field, entry) in &mut fields {
                if !file.materialized_field_matches(field, entry) {
                    continue;
                }
                affected.insert(field.clone());
                entry.input_fingerprint = None;
                entry.value_json = None;
                entry.materialized_value_json = None;
                entry.diagnostic = Some(ComputedFieldDiagnostic {
                    module: entry.module.clone(),
                    field: field.clone(),
                    code: "dependency_changed".to_string(),
                    message: message.to_string(),
                    span_start: None,
                    span_end: None,
                });
            }
            if affected.is_empty() {
                continue;
            }
            let owning_modules: BTreeSet<String> = file
                .computed_fields
                .values()
                .map(|entry| entry.module.clone())
                .collect();
            let materialized: BTreeSet<String> = owning_modules
                .iter()
                .flat_map(|module| materialized_fields_for_module(&file, module))
                .filter(|field| affected.contains(field))
                .collect();
            if !materialized.is_empty() {
                let root_guard = || lock.verify_project_root(project_root);
                if let Ok(writeback) =
                    crate::frontmatter_write::apply_frontmatter_patch_with_intent_and_guard(
                        project_root,
                        Path::new(&path),
                        &file.content_hash,
                        &BTreeMap::new(),
                        &materialized,
                        crate::frontmatter_write::ComputedWriteContext {
                            owned_unset: &materialized,
                            fields: &fields,
                            pre_commit_guard: Some(&root_guard),
                        },
                    )
                {
                    index.apply_module_source_state(&file.content_hash, &writeback.file, fields)?;
                    continue;
                }
            }
            restore_materialized_proofs(&mut fields, &file.computed_fields, &materialized);
            // A concurrent owner edit wins. Diagnostics still make every
            // stale materialization ineffective until the next retry.
            index.replace_computed_fields(&path, fields)?;
        }
        Ok(())
    }

    fn unstable_schema_reports(
        &self,
        project_root: &Path,
        index: &Index,
        event: &ModuleEvent,
        lock: &ModuleRunLock,
    ) -> crate::Result<Vec<ModuleReport>> {
        let message = "schema overlay kept changing during computed-module evaluation; computed values were suppressed for retry";
        Self::suppress_for_unstable_schema(project_root, index, lock, message)?;
        // Persist raw ingest state and fail-closed ownership diagnostics, never
        // a set of apparently valid values produced from mixed definitions.
        Self::save_and_finish_writebacks(project_root, index, lock)?;
        Ok(self
            .modules
            .iter()
            .map(|module| {
                let descriptor = module.descriptor();
                ModuleReport {
                    module: descriptor.id.clone(),
                    event: event.name().to_string(),
                    files_evaluated: 0,
                    fields_updated: 0,
                    diagnostics: Self::failure_execution(&descriptor.id, message.to_string())
                        .diagnostics,
                    duration_ms: 0,
                }
            })
            .collect())
    }

    pub fn run(
        &self,
        project_root: &Path,
        index: &Index,
        event: &ModuleEvent,
    ) -> crate::Result<Vec<ModuleReport>> {
        let lock = acquire_module_run_lock(project_root)?;
        index.reload_from_disk_if_clean()?;
        self.run_locked(project_root, index, event, &lock)
    }

    /// Run after the caller has serialized the complete raw-index/module
    /// mutation lifecycle for this project. Ingest and watch use this entry
    /// point so another process cannot branch from an older index generation
    /// before waiting for the computed-module lock.
    pub(crate) fn run_locked(
        &self,
        project_root: &Path,
        index: &Index,
        event: &ModuleEvent,
        lock: &ModuleRunLock,
    ) -> crate::Result<Vec<ModuleReport>> {
        lock.verify_project_root(project_root)?;
        let recovering = Self::recover_pending_writebacks(project_root, index, lock)?;
        let recovery_event = ModuleEvent::FullIngest;
        let event = if recovering { &recovery_event } else { event };
        for attempt in 0..3 {
            lock.verify_project_root(project_root)?;
            let schema_before = Schema::overlay_source_fingerprint(project_root)?;
            let mut reports = Vec::with_capacity(self.modules.len());
            let mut stable = true;
            for module in &self.modules {
                if !Self::schema_matches(project_root, &schema_before, lock)? {
                    stable = false;
                    break;
                }
                reports.push(Self::execute(
                    module.as_ref(),
                    project_root,
                    index,
                    event,
                    lock,
                ));
                if !Self::schema_matches(project_root, &schema_before, lock)? {
                    stable = false;
                    break;
                }
            }
            if stable {
                // Persist raw ingest state and both ordered module results while
                // the same cross-process lock is still held.
                Self::save_and_finish_writebacks(project_root, index, lock)?;
                return Ok(reports);
            }
            if attempt == 2 {
                return self.unstable_schema_reports(project_root, index, event, lock);
            }
        }
        unreachable!("schema convergence loop either returns or retries")
    }

    /// Run an ordered dependency pipeline while holding one project-wide lock.
    pub fn run_pipeline(
        &self,
        project_root: &Path,
        index: &Index,
        runs: &[(&str, &ModuleEvent)],
    ) -> crate::Result<Vec<ModuleReport>> {
        let lock = acquire_module_run_lock(project_root)?;
        lock.verify_project_root(project_root)?;
        index.reload_from_disk_if_clean()?;
        Self::refresh_manual_sources(project_root, index, &lock)?;
        let recovering = Self::recover_pending_writebacks(project_root, index, &lock)?;
        let recovery_event = ModuleEvent::FullIngest;

        for attempt in 0..3 {
            lock.verify_project_root(project_root)?;
            let schema_before = Schema::overlay_source_fingerprint(project_root)?;
            let mut reports = Vec::with_capacity(runs.len());
            let mut stable = true;
            for (id, event) in runs {
                let event = if recovering { &recovery_event } else { *event };
                if !Self::schema_matches(project_root, &schema_before, &lock)? {
                    stable = false;
                    break;
                }
                if let Some(module) = self
                    .modules
                    .iter()
                    .find(|module| module.descriptor().id == *id)
                {
                    reports.push(Self::execute(
                        module.as_ref(),
                        project_root,
                        index,
                        event,
                        &lock,
                    ));
                }
                if !Self::schema_matches(project_root, &schema_before, &lock)? {
                    stable = false;
                    break;
                }
            }
            if stable {
                Self::save_and_finish_writebacks(project_root, index, &lock)?;
                return Ok(reports);
            }
            if attempt == 2 {
                let fallback_event = ModuleEvent::ManualRun { scope: None };
                let event = runs
                    .first()
                    .map(|(_, event)| *event)
                    .unwrap_or(&fallback_event);
                return self.unstable_schema_reports(project_root, index, event, &lock);
            }
        }
        unreachable!("schema convergence loop either returns or retries")
    }

    /// Plan and execute the exact prerequisite/downstream closure for a manual
    /// module request. Output writes remain limited to requested owner paths,
    /// Formula leaves consumed by Lookup/Rollup, and downstream owners that
    /// consume a requested Formula result.
    pub fn run_dependency_aware(
        &self,
        project_root: &Path,
        index: &Index,
        requested_module: &str,
        scope: Option<&str>,
    ) -> crate::Result<Vec<ModuleReport>> {
        let lock = acquire_module_run_lock(project_root)?;
        self.run_dependency_aware_locked(project_root, index, requested_module, scope, &lock)
    }

    /// Execute the dependency-aware pipeline under an already-held project
    /// lock. Used by the desktop overlay transaction protocol so no ingest or
    /// watcher can observe the new definition before it is materialized.
    pub fn run_dependency_aware_locked(
        &self,
        project_root: &Path,
        index: &Index,
        requested_module: &str,
        scope: Option<&str>,
        lock: &ModuleRunLock,
    ) -> crate::Result<Vec<ModuleReport>> {
        lock.verify_project_root(project_root)?;
        index.reload_from_disk_if_clean()?;
        Self::refresh_manual_sources(project_root, index, lock)?;
        let recovering = Self::recover_pending_writebacks(project_root, index, lock)?;

        for attempt in 0..3 {
            lock.verify_project_root(project_root)?;
            let schema_before = Schema::overlay_source_fingerprint(project_root)?;
            let files = index.get_all_files();
            // Dependency planning is an optimization over a valid overlay,
            // not a prerequisite for fail-closed module execution.  Each
            // module owns the invalid-schema path that removes stale
            // materializations and persists diagnostics.  If planning were to
            // return the overlay error here, a manual run would skip that
            // cleanup entirely and leave the previous values authoritative.
            let overlay = Schema::load_overlay(project_root).ok().flatten();
            let requested_event = ModuleEvent::ManualRun {
                scope: scope.map(str::to_string),
            };
            let global_event = ModuleEvent::ManualRun { scope: None };
            let plan = overlay.as_ref().map(|overlay| {
                lookup_rollup::plan_manual_dependencies(&files, overlay, requested_module, scope)
            });
            let mut formula_event = plan.as_ref().map_or_else(
                || {
                    if requested_module == FORMULA_MODULE_ID {
                        requested_event.clone()
                    } else {
                        global_event.clone()
                    }
                },
                |plan| ModuleEvent::ManualPaths {
                    paths: plan.formula_paths.clone(),
                },
            );
            let mut lookup_rollup_event = plan.as_ref().map_or_else(
                || {
                    if requested_module == LOOKUP_ROLLUP_MODULE_ID {
                        requested_event.clone()
                    } else {
                        global_event.clone()
                    }
                },
                |plan| ModuleEvent::ManualPaths {
                    paths: plan.lookup_rollup_paths.clone(),
                },
            );
            // A missing or malformed overlay is a collection-wide definition
            // failure, not a scoped data change. Persisted ownership outside
            // the requested output lens would otherwise keep stale values
            // authoritative until some later unscoped event. Let both modules
            // perform their fail-closed cleanup across the full snapshot.
            if recovering || overlay.is_none() {
                formula_event = ModuleEvent::FullIngest;
                lookup_rollup_event = ModuleEvent::FullIngest;
            }
            let ordered = [
                (FORMULA_MODULE_ID, &formula_event),
                (LOOKUP_ROLLUP_MODULE_ID, &lookup_rollup_event),
            ];
            let mut reports = Vec::with_capacity(ordered.len());
            let mut stable = true;
            for (id, event) in ordered {
                if !Self::schema_matches(project_root, &schema_before, lock)? {
                    stable = false;
                    break;
                }
                if let Some(module) = self
                    .modules
                    .iter()
                    .find(|module| module.descriptor().id == id)
                {
                    reports.push(Self::execute(
                        module.as_ref(),
                        project_root,
                        index,
                        event,
                        lock,
                    ));
                }
                if !Self::schema_matches(project_root, &schema_before, lock)? {
                    stable = false;
                    break;
                }
            }
            if stable {
                Self::save_and_finish_writebacks(project_root, index, lock)?;
                return Ok(reports);
            }
            if attempt == 2 {
                return self.unstable_schema_reports(
                    project_root,
                    index,
                    &ModuleEvent::ManualRun {
                        scope: scope.map(str::to_string),
                    },
                    lock,
                );
            }
        }
        unreachable!("schema convergence loop either returns or retries")
    }

    fn refresh_manual_sources(
        project_root: &Path,
        index: &Index,
        lock: &ModuleRunLock,
    ) -> crate::Result<()> {
        lock.verify_project_root(project_root)?;
        // Manual runs double as watcher-off catch-up. Reconcile the complete
        // discovered Markdown membership while holding the project module lock;
        // the requested path/Shard is an output lens, never a dependency read
        // boundary. New files are inserted as provisional source metadata only
        // (no chunking/embedding), and the sentinel installed by the index
        // guarantees that a later ingest still embeds their body.
        let config = crate::config::Config::load(project_root)?;
        let mut discovered =
            crate::discovery::FileDiscovery::new(project_root, &config).discover()?;
        discovered.sort();

        let indexed = index.get_all_files();
        let discovered_paths: BTreeSet<String> = discovered
            .iter()
            .map(|path| crate::path_util::to_slash(path))
            .collect();

        // Parse before mutating membership. A malformed/unreadable file keeps
        // an existing snapshot (whose dependency CAS will fail closed) and is
        // never introduced as a plausible new relation target.
        let parsed: Vec<_> = discovered
            .iter()
            .filter_map(|relative| {
                crate::parser::parse_markdown_file(project_root, relative)
                    .ok()
                    .map(|file| (crate::path_util::to_slash(relative), file))
            })
            .collect();

        lock.verify_project_root(project_root)?;
        let mut removed: Vec<_> = indexed
            .keys()
            .filter(|path| !discovered_paths.contains(*path))
            .cloned()
            .collect();
        removed.sort();
        let inserts_membership = parsed.iter().any(|(path, _)| !indexed.contains_key(path));
        if !removed.is_empty() || inserts_membership {
            // Vector/source membership and the lexical projection are companion
            // stores. The owning MarkdownVdb run repairs and clears this marker
            // after the module pipeline saves; a crash or direct runner caller
            // leaves it for the next writable open to self-heal.
            crate::fts::begin_reconciliation(project_root)?;
        }
        for path in removed {
            index.remove_file(&path)?;
        }

        for (path, file) in &parsed {
            match indexed.get(path) {
                Some(stored) if file.content_hash != stored.content_hash => {
                    index.refresh_source_metadata(file)?;
                }
                Some(_) => {}
                None => index.insert_unembedded_source_metadata(file)?,
            }
        }

        // The manual runner is the watcher-off catch-up path and may be the
        // first operation after an incompatible derived index self-heals to an
        // empty index. Rebuild the complete raw schema from the files already
        // parsed above before computed modules sanitize materialized keys and
        // reapply the overlay. Otherwise an empty cached schema would retain
        // only overlay/computed fields and hide ordinary Lookup targets such as
        // `clients.domain` from schema consumers.
        let files: Vec<_> = parsed.iter().map(|(_, file)| file.clone()).collect();
        let overlay = Schema::load_overlay(project_root).ok().flatten();
        let inferred = Schema::infer(&files);
        index.set_schema(Some(Schema::merge(
            inferred,
            overlay.as_ref().map(|overlay| overlay.fields.clone()),
        )));

        let mut scopes = BTreeSet::new();
        scopes.extend(Schema::discover_scopes(&files));
        if let Some(overlay) = &overlay {
            scopes.extend(overlay.scopes.keys().cloned());
        }
        if let Ok(shards) = crate::shards::ShardStore::new(project_root).list() {
            scopes.extend(shards.shards.into_iter().map(|shard| shard.path));
        }
        let scoped_schemas: Vec<_> = scopes
            .into_iter()
            .map(|scope| {
                let inferred = Schema::infer_scoped(&files, &scope);
                let overlay_fields = overlay
                    .as_ref()
                    .map(|overlay| Schema::resolve_overlay_for_path(overlay, Some(&scope)));
                crate::schema::ScopedSchema {
                    scope,
                    schema: Schema::merge(inferred, overlay_fields),
                }
            })
            .collect();
        index.set_scoped_schemas((!scoped_schemas.is_empty()).then_some(scoped_schemas));
        lock.verify_project_root(project_root)
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
        let lock = match acquire_module_run_lock(project_root) {
            Ok(lock) => lock,
            Err(error) => {
                return Some(Self::lock_failure_report(
                    module.as_ref(),
                    event,
                    error.to_string(),
                ));
            }
        };
        if let Err(error) = lock.verify_project_root(project_root) {
            return Some(Self::lock_failure_report(
                module.as_ref(),
                event,
                error.to_string(),
            ));
        }
        if let Err(error) = index.reload_from_disk_if_clean() {
            return Some(Self::lock_failure_report(
                module.as_ref(),
                event,
                error.to_string(),
            ));
        }
        let recovering = match Self::recover_pending_writebacks(project_root, index, &lock) {
            Ok(recovering) => recovering,
            Err(error) => {
                return Some(Self::lock_failure_report(
                    module.as_ref(),
                    event,
                    error.to_string(),
                ));
            }
        };
        let recovery_event = ModuleEvent::FullIngest;
        let event = if recovering { &recovery_event } else { event };
        let report = Self::execute(module.as_ref(), project_root, index, event, &lock);
        if let Err(error) = Self::save_and_finish_writebacks(project_root, index, &lock) {
            return Some(Self::lock_failure_report(
                module.as_ref(),
                event,
                error.to_string(),
            ));
        }
        Some(report)
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

    fn input_fingerprint(inputs: &JsonMap<String, JsonValue>) -> String {
        let mut hasher = Sha256::new();
        hasher.update(b"formula_inputs_v1");
        for (field, value) in inputs.iter().collect::<BTreeMap<_, _>>() {
            hasher.update([0]);
            hasher.update(field.as_bytes());
            hasher.update([0]);
            if let Ok(encoded) = serde_json::to_vec(value) {
                hasher.update(encoded);
            }
        }
        format!("{:x}", hasher.finalize())
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
        overlay: &OverlaySchema,
        program: &FormulaProgram,
        execution: &mut ModuleExecution,
    ) {
        let mut inputs = Self::input_fields(path, file);
        for definition in program.definitions() {
            inputs.remove(&definition.name);
        }
        let mut unsupported_inputs: BTreeSet<String> = file
            .computed_fields
            .iter()
            .filter(|(field, entry)| {
                entry.module != FORMULA_MODULE_ID && file.materialized_field_matches(field, entry)
            })
            .map(|(field, _)| field.clone())
            .collect();
        // A Formula may depend on other Formula fields in the same compiled
        // program, but it must never consume a materialized value owned by a
        // different module. In particular, Lookup/Rollup runs after Formula and
        // cross-module back-edges would otherwise make results event-order
        // dependent rather than a deterministic DAG.
        for (field, entry) in &file.computed_fields {
            if file.materialized_field_matches(field, entry) {
                inputs.remove(field);
            }
        }
        // A freshly rebuilt index may not yet have module ownership metadata,
        // while source frontmatter can still contain a prior Lookup/Rollup
        // materialization. The effective overlay is therefore authoritative
        // for excluding cross-module inputs as well as the cache above.
        for (field, definition) in Schema::resolve_overlay_for_path(overlay, Some(path)) {
            if definition
                .field_type
                .as_deref()
                .and_then(crate::schema::parse_field_type_str)
                .is_some_and(|field_type| {
                    matches!(field_type, FieldType::Lookup | FieldType::Rollup)
                })
            {
                unsupported_inputs.insert(field.clone());
                inputs.remove(&field);
            }
        }
        let input_fingerprint = Some(Self::input_fingerprint(&inputs));
        // The owner is already guarded by the patch's full-source CAS. Keep
        // its existence in provenance, but not the pre-materialization hash:
        // the module's own successful write necessarily changes that hash.
        let dependency_snapshot = ComputedDependencySnapshot::owner_source(path);
        let mut evaluation = program.evaluate(&inputs);
        for definition in program.definitions() {
            let Some(dependency) =
                program
                    .dependencies_for(&definition.name)
                    .and_then(|dependencies| {
                        dependencies
                            .iter()
                            .find(|dependency| unsupported_inputs.contains(*dependency))
                    })
            else {
                continue;
            };
            evaluation.values.remove(&definition.name);
            evaluation.errors.insert(
                definition.name.clone(),
                FormulaDiagnostic {
                    module: FORMULA_MODULE_ID.to_string(),
                    field: definition.name.clone(),
                    code: "unsupported_dependency".to_string(),
                    message: format!(
                        "Formula field `{}` cannot consume Lookup/Rollup field `{dependency}`",
                        definition.name
                    ),
                    span: None,
                },
            );
        }
        let mut entries = file.computed_fields.clone();
        let mut frontmatter_unset: BTreeSet<String> = entries
            .iter()
            .filter(|(field, entry)| {
                entry.module == FORMULA_MODULE_ID && file.materialized_field_matches(field, entry)
            })
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
            let materialized_value_json = value_json.clone();

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
                    input_fingerprint: input_fingerprint.clone(),
                    dependency_snapshot: dependency_snapshot.clone(),
                    value_json,
                    materialized_value_json,
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
            let frontmatter_unset = previous_fields
                .iter()
                .filter(|field| {
                    file.computed_fields
                        .get(*field)
                        .is_some_and(|entry| file.materialized_field_matches(field, entry))
                })
                .cloned()
                .collect();

            if previous_fields.is_empty() {
                execution.diagnostics.push(ModuleDiagnostic {
                    path: Some(path.clone()),
                    module: FORMULA_MODULE_ID.to_string(),
                    field: String::new(),
                    code: "invalid_schema".to_string(),
                    message: message.to_string(),
                    span_start: None,
                    span_end: None,
                });
                continue;
            }

            for field in previous_fields {
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
                        input_fingerprint: None,
                        dependency_snapshot: ComputedDependencySnapshot::default(),
                        value_json: None,
                        materialized_value_json: None,
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
            let frontmatter_unset = previous_fields
                .iter()
                .filter(|field| {
                    file.computed_fields
                        .get(*field)
                        .is_some_and(|entry| file.materialized_field_matches(field, entry))
                })
                .cloned()
                .collect();

            for field in previous_fields {
                let message =
                    "the schema overlay was removed; the cached formula value was cleared";
                entries.insert(
                    field.clone(),
                    ComputedFieldEntry {
                        module: FORMULA_MODULE_ID.to_string(),
                        definition_fingerprint: String::new(),
                        input_fingerprint: None,
                        dependency_snapshot: ComputedDependencySnapshot::default(),
                        value_json: None,
                        materialized_value_json: None,
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
        rebuild_schema_without_computed_materializations(context, overlay)
    }

    fn refresh_one_schema<'a>(
        schema: &mut Schema,
        files: impl IntoIterator<Item = &'a StoredFile>,
    ) {
        refresh_computed_schema_stats(schema, files);
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
        let pre_load_schema_fingerprint =
            Schema::overlay_source_fingerprint(context.project_root).ok();
        let (overlay, schema_fingerprint) =
            match Schema::load_overlay_with_fingerprint(context.project_root) {
                Ok(snapshot) => snapshot,
                Err(error) => {
                    execution.expected_schema_fingerprint = pre_load_schema_fingerprint;
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
        execution.expected_schema_fingerprint = Some(schema_fingerprint);
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
                        .filter(|(field, entry)| file.materialized_field_matches(field, entry))
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
            let effective_overlay = overlay
                .as_ref()
                .expect("a non-empty Formula program requires a loaded overlay");
            self.evaluate_file(&path, file, effective_overlay, program, &mut execution);
        }
        Self::finish_schema_patch(context, overlay.as_ref(), &mut execution);
        Ok(execution)
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
                    .is_none_or(|scope| Self::path_matches_scope(path, scope))
                    .then_some(file)
            }),
        );
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
        ModuleEvent::ManualPaths { paths } => paths
            .iter()
            .filter(|path| files.contains_key(*path))
            .cloned()
            .collect(),
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
    fn schema_refresh_drops_an_empty_persisted_scope_without_an_overlay_definition() {
        let dir = TempDir::new().unwrap();
        let files = HashMap::new();
        let empty_schema = Schema::infer_from_frontmatter_iter(std::iter::empty::<&JsonValue>());
        let scoped = [crate::schema::ScopedSchema {
            scope: "deleted".to_string(),
            schema: empty_schema,
        }];
        let context = ModuleContext {
            project_root: dir.path(),
            files: &files,
            schema: None,
            scoped_schemas: Some(&scoped),
        };

        let patch = rebuild_schema_without_computed_materializations(&context, None);
        assert!(patch.scoped_schemas.is_none());
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
                            input_fingerprint: None,
                            dependency_snapshot: ComputedDependencySnapshot::default(),
                            value_json: Some(formula.to_string()),
                            materialized_value_json: Some(formula.to_string()),
                            diagnostic: None,
                        },
                    )]),
                )
                .unwrap();
        }
        index.save().unwrap();

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

    #[test]
    fn formula_rejects_overlay_computed_input_without_cached_ownership() {
        let dir = TempDir::new().unwrap();
        std::fs::write(
            dir.path().join(".markdownvdb.schema.yml"),
            r#"scopes:
  docs:
    fields:
      cached_lookup:
        field_type: lookup
        relation_field: peer
        target_field: score
      formula_total:
        field_type: formula
        formula: cached_lookup + 1
        result_type: number
"#,
        )
        .unwrap();
        std::fs::create_dir_all(dir.path().join("docs")).unwrap();
        std::fs::write(
            dir.path().join("docs/a.md"),
            "---\ncached_lookup: 10\nformula_total: 11\n---\nBody\n",
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
            crate::parser::parse_markdown_file(dir.path(), Path::new("docs/a.md")).unwrap();
        index.upsert(&parsed, &[], &[]).unwrap();
        index.save().unwrap();
        assert!(index.get_computed_fields("docs/a.md").unwrap().is_empty());

        let report = ModuleRunner::builtins()
            .run_one(
                FORMULA_MODULE_ID,
                dir.path(),
                &index,
                &ModuleEvent::FullIngest,
            )
            .unwrap();

        assert!(report
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == "unsupported_dependency"));
        assert!(report
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == "writeback_failed"));
        let rewritten = crate::parser::parse_markdown_file(dir.path(), Path::new("docs/a.md"))
            .unwrap()
            .frontmatter
            .unwrap();
        assert_eq!(rewritten["cached_lookup"], JsonValue::from(10));
        // No prior materialization proof exists for this colliding ordinary
        // key, so even an evaluation failure may not delete it.
        assert_eq!(rewritten["formula_total"], JsonValue::from(11));
    }

    #[test]
    fn run_one_commits_source_provenance_and_clears_the_write_intent() {
        let dir = TempDir::new().unwrap();
        let state_dir = dir.path().join(".markdownvdb");
        std::fs::create_dir_all(&state_dir).unwrap();
        std::fs::write(
            dir.path().join(".markdownvdb.schema.yml"),
            "fields:\n  total:\n    field_type: formula\n    formula: price * 2\n    result_type: number\n",
        )
        .unwrap();
        std::fs::write(dir.path().join("invoice.md"), "---\nprice: 2\n---\nBody\n").unwrap();

        let index_path = state_dir.join("index");
        let index = Index::create(
            &index_path,
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
        index.save().unwrap();

        let report = ModuleRunner::builtins()
            .run_one(
                FORMULA_MODULE_ID,
                dir.path(),
                &index,
                &ModuleEvent::FullIngest,
            )
            .unwrap();
        assert!(report.diagnostics.is_empty(), "{:?}", report.diagnostics);
        assert!(!state_dir.join("computed-write-intent.json").exists());

        drop(index);
        let reopened = Index::open(&index_path).unwrap();
        let stored = reopened.get_file("invoice.md").unwrap();
        let source =
            crate::parser::parse_markdown_file(dir.path(), Path::new("invoice.md")).unwrap();
        assert_eq!(stored.content_hash, source.content_hash);
        assert_eq!(
            stored.computed_fields["total"].value_json.as_deref(),
            Some("4")
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

    struct NoopModule;

    impl Module for NoopModule {
        fn descriptor(&self) -> ModuleDescriptor {
            ModuleDescriptor {
                id: "noop".to_string(),
                name: "No-op test module".to_string(),
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
            Ok(ModuleExecution::default())
        }
    }

    #[cfg(unix)]
    struct RootRetargetModule {
        project: std::path::PathBuf,
        relocated: std::path::PathBuf,
        replacement: std::path::PathBuf,
        replacement_lock: std::sync::Mutex<Option<ModuleRunLock>>,
    }

    #[cfg(unix)]
    impl Module for RootRetargetModule {
        fn descriptor(&self) -> ModuleDescriptor {
            ModuleDescriptor {
                id: "root_retarget".to_string(),
                name: "Root retarget test module".to_string(),
                version: 1,
                always_on: true,
                hooks: vec!["manual_run".to_string()],
            }
        }

        fn run(
            &self,
            context: &ModuleContext<'_>,
            _event: &ModuleEvent,
        ) -> crate::Result<ModuleExecution> {
            use std::os::unix::fs::symlink;

            let owner = &context.files["owner.md"];
            std::fs::rename(&self.project, &self.relocated)?;
            symlink(&self.replacement, &self.project)?;

            // This succeeds because the pathname now resolves to a second
            // vault. Keep that second lock held while the original execution
            // attempts its write, reproducing the split-lock hazard exactly.
            let replacement_lock = acquire_module_run_lock(&self.project)?;
            *self.replacement_lock.lock().unwrap() = Some(replacement_lock);

            let field = "computed".to_string();
            Ok(ModuleExecution {
                files_evaluated: 1,
                fields_updated: 1,
                derived_field_patches: vec![DerivedFieldPatch {
                    path: "owner.md".to_string(),
                    expected_content_hash: owner.content_hash.clone(),
                    fields: HashMap::from([(
                        field.clone(),
                        ComputedFieldEntry {
                            module: "root_retarget".to_string(),
                            definition_fingerprint: "definition".to_string(),
                            input_fingerprint: Some("inputs".to_string()),
                            dependency_snapshot: ComputedDependencySnapshot::default(),
                            value_json: Some("2".to_string()),
                            materialized_value_json: Some("2".to_string()),
                            diagnostic: None,
                        },
                    )]),
                    frontmatter_set: BTreeMap::from([(field.clone(), JsonValue::from(2))]),
                    frontmatter_unset: BTreeSet::from([field]),
                }],
                ..ModuleExecution::default()
            })
        }
    }

    #[test]
    fn module_transaction_rejects_an_unsaved_partial_index_branch() {
        let dir = TempDir::new().unwrap();
        let index_path = dir.path().join("index.bin");
        let index = Index::create(
            &index_path,
            &EmbeddingConfig {
                provider: "test".to_string(),
                model: "test".to_string(),
                dimensions: 2,
            },
        )
        .unwrap();
        std::fs::write(dir.path().join("partial.md"), "Partial\n").unwrap();
        let partial =
            crate::parser::parse_markdown_file(dir.path(), Path::new("partial.md")).unwrap();
        index.upsert(&partial, &[], &[]).unwrap();

        let report = ModuleRunner::new(vec![Box::new(NoopModule)])
            .run_one(
                "noop",
                dir.path(),
                &index,
                &ModuleEvent::ManualRun { scope: None },
            )
            .unwrap();
        assert_eq!(report.diagnostics[0].code, "module_error");
        assert!(report.diagnostics[0]
            .message
            .contains("unsaved in-memory changes"));
        assert!(Index::open(&index_path)
            .unwrap()
            .get_file("partial.md")
            .is_none());
    }

    #[test]
    fn project_module_lock_rejects_a_concurrent_run() {
        let dir = TempDir::new().unwrap();
        std::fs::create_dir_all(dir.path().join(".markdownvdb")).unwrap();
        let lock_path = dir.path().join(".markdownvdb/modules.lock");
        let external_lock = std::fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(&lock_path)
            .unwrap();
        external_lock.lock().unwrap();
        let index = Index::create(
            &dir.path().join("index.bin"),
            &EmbeddingConfig {
                provider: "test".to_string(),
                model: "test".to_string(),
                dimensions: 2,
            },
        )
        .unwrap();

        let report = ModuleRunner::new(vec![Box::new(NoopModule)])
            .run_one(
                "noop",
                dir.path(),
                &index,
                &ModuleEvent::ManualRun { scope: None },
            )
            .unwrap();
        assert_eq!(report.diagnostics[0].code, "module_error");
        assert!(report.diagnostics[0]
            .message
            .contains("another computed-module run"));

        drop(external_lock);
        let report = ModuleRunner::new(vec![Box::new(NoopModule)])
            .run_one(
                "noop",
                dir.path(),
                &index,
                &ModuleEvent::ManualRun { scope: None },
            )
            .unwrap();
        assert!(report.diagnostics.is_empty());
    }

    #[cfg(unix)]
    #[test]
    fn project_module_lock_rejects_symlinked_state_without_writing_outside() {
        use std::os::unix::fs::symlink;

        let project = TempDir::new().unwrap();
        let outside = TempDir::new().unwrap();
        symlink(outside.path(), project.path().join(".markdownvdb")).unwrap();

        let result = acquire_module_run_lock(project.path());

        assert!(matches!(result, Err(Error::Config(_))));
        assert!(!outside.path().join("modules.lock").exists());
        assert_eq!(std::fs::read_dir(outside.path()).unwrap().count(), 0);
    }

    #[cfg(unix)]
    #[test]
    fn project_module_lock_rejects_symlinked_lock_without_touching_target() {
        use std::os::unix::fs::symlink;

        let project = TempDir::new().unwrap();
        let outside = TempDir::new().unwrap();
        let state = project.path().join(".markdownvdb");
        let outside_lock = outside.path().join("outside.lock");
        std::fs::create_dir(&state).unwrap();
        std::fs::write(&outside_lock, b"sentinel").unwrap();
        symlink(&outside_lock, state.join("modules.lock")).unwrap();

        let result = acquire_module_run_lock(project.path());

        assert!(matches!(result, Err(Error::Config(_))));
        assert_eq!(std::fs::read(&outside_lock).unwrap(), b"sentinel");
        assert!(state.join("modules.lock").is_symlink());
    }

    #[cfg(unix)]
    #[test]
    fn root_retarget_during_module_evaluation_never_writes_under_split_lock() {
        let container = TempDir::new().unwrap();
        let replacement = TempDir::new().unwrap();
        let project = container.path().join("project");
        let relocated = container.path().join("project-relocated");
        std::fs::create_dir(&project).unwrap();
        let original = "---\ntitle: Original\n---\nBody\n";
        let replacement_original = "---\ntitle: Replacement\n---\nOther body\n";
        std::fs::write(project.join("owner.md"), original).unwrap();
        std::fs::write(replacement.path().join("owner.md"), replacement_original).unwrap();

        let index = Index::create(
            &container.path().join("index.bin"),
            &EmbeddingConfig {
                provider: "test".to_string(),
                model: "test".to_string(),
                dimensions: 2,
            },
        )
        .unwrap();
        let parsed = crate::parser::parse_markdown_file(&project, Path::new("owner.md")).unwrap();
        index.upsert(&parsed, &[], &[]).unwrap();
        index.save().unwrap();

        let runner = ModuleRunner::new(vec![Box::new(RootRetargetModule {
            project: project.clone(),
            relocated: relocated.clone(),
            replacement: replacement.path().to_path_buf(),
            replacement_lock: std::sync::Mutex::new(None),
        })]);
        let report = runner
            .run_one(
                "root_retarget",
                &project,
                &index,
                &ModuleEvent::ManualRun { scope: None },
            )
            .unwrap();

        assert_eq!(report.fields_updated, 0);
        assert_eq!(report.diagnostics[0].code, "module_error");
        assert!(report.diagnostics[0]
            .message
            .contains("project root changed while the computed-module lock was held"));
        assert_eq!(
            std::fs::read_to_string(relocated.join("owner.md")).unwrap(),
            original
        );
        assert_eq!(
            std::fs::read_to_string(replacement.path().join("owner.md")).unwrap(),
            replacement_original
        );
        assert!(relocated.join(".markdownvdb/modules.lock").is_file());
        assert!(replacement
            .path()
            .join(".markdownvdb/modules.lock")
            .is_file());
        assert!(!replacement
            .path()
            .join(".markdownvdb/computed-write-intent.json")
            .exists());
        assert!(index.get_computed_fields("owner.md").unwrap().is_empty());
    }

    struct DependencyGuardModule;

    impl Module for DependencyGuardModule {
        fn descriptor(&self) -> ModuleDescriptor {
            ModuleDescriptor {
                id: "dependency_guard".to_string(),
                name: "Dependency guard test module".to_string(),
                version: 1,
                always_on: true,
                hooks: vec!["manual_run".to_string()],
            }
        }

        fn run(
            &self,
            context: &ModuleContext<'_>,
            _event: &ModuleEvent,
        ) -> crate::Result<ModuleExecution> {
            let owner = &context.files["owner.md"];
            let field = "derived".to_string();
            let value = JsonValue::String("unsafe".to_string());
            Ok(ModuleExecution {
                files_evaluated: 1,
                fields_updated: 1,
                derived_field_patches: vec![DerivedFieldPatch {
                    path: "owner.md".to_string(),
                    expected_content_hash: owner.content_hash.clone(),
                    fields: HashMap::from([(
                        field.clone(),
                        ComputedFieldEntry {
                            module: "dependency_guard".to_string(),
                            definition_fingerprint: "definition".to_string(),
                            input_fingerprint: None,
                            dependency_snapshot: ComputedDependencySnapshot::default(),
                            value_json: Some("\"unsafe\"".to_string()),
                            materialized_value_json: Some("\"unsafe\"".to_string()),
                            diagnostic: None,
                        },
                    )]),
                    frontmatter_set: BTreeMap::from([(field.clone(), value)]),
                    frontmatter_unset: BTreeSet::from([field]),
                }],
                expected_dependency_hashes: BTreeMap::from([(
                    "dependency.md".to_string(),
                    "stale-hash".to_string(),
                )]),
                ..ModuleExecution::default()
            })
        }
    }

    struct IncomingMembershipGuardModule;

    impl Module for IncomingMembershipGuardModule {
        fn descriptor(&self) -> ModuleDescriptor {
            ModuleDescriptor {
                id: "incoming_membership_guard".to_string(),
                name: "Incoming membership guard test module".to_string(),
                version: 1,
                always_on: true,
                hooks: vec!["manual_run".to_string()],
            }
        }

        fn run(
            &self,
            context: &ModuleContext<'_>,
            _event: &ModuleEvent,
        ) -> crate::Result<ModuleExecution> {
            let owner = &context.files["owner.md"];
            let field = "derived".to_string();
            Ok(ModuleExecution {
                files_evaluated: 1,
                fields_updated: 1,
                derived_field_patches: vec![DerivedFieldPatch {
                    path: "owner.md".to_string(),
                    expected_content_hash: owner.content_hash.clone(),
                    fields: HashMap::from([(
                        field.clone(),
                        ComputedFieldEntry {
                            module: "incoming_membership_guard".to_string(),
                            definition_fingerprint: "definition".to_string(),
                            input_fingerprint: None,
                            dependency_snapshot: ComputedDependencySnapshot::default(),
                            value_json: Some("\"unsafe\"".to_string()),
                            materialized_value_json: Some("\"unsafe\"".to_string()),
                            diagnostic: None,
                        },
                    )]),
                    frontmatter_set: BTreeMap::from([(
                        field.clone(),
                        JsonValue::String("unsafe".to_string()),
                    )]),
                    frontmatter_unset: BTreeSet::from([field]),
                }],
                expected_incoming_scope_membership: BTreeMap::from([(
                    "invoices".to_string(),
                    BTreeSet::new(),
                )]),
                ..ModuleExecution::default()
            })
        }
    }

    #[test]
    fn incoming_membership_config_error_aborts_owner_write() {
        let dir = TempDir::new().unwrap();
        std::fs::create_dir_all(dir.path().join(".markdownvdb")).unwrap();
        std::fs::write(dir.path().join(".markdownvdb/config.yaml"), "sources: [").unwrap();
        let original = "---\nname: owner\n---\nOwner\n";
        std::fs::write(dir.path().join("owner.md"), original).unwrap();

        let index = Index::create(
            &dir.path().join("index.bin"),
            &EmbeddingConfig {
                provider: "test".to_string(),
                model: "test".to_string(),
                dimensions: 2,
            },
        )
        .unwrap();
        let parsed = crate::parser::parse_markdown_file(dir.path(), Path::new("owner.md")).unwrap();
        index.upsert(&parsed, &[], &[]).unwrap();
        index.save().unwrap();

        let report = ModuleRunner::new(vec![Box::new(IncomingMembershipGuardModule)])
            .run_one(
                "incoming_membership_guard",
                dir.path(),
                &index,
                &ModuleEvent::ManualRun { scope: None },
            )
            .unwrap();

        assert_eq!(report.fields_updated, 0);
        assert_eq!(report.diagnostics.len(), 1);
        assert_eq!(report.diagnostics[0].code, "module_error");
        assert!(report.diagnostics[0]
            .message
            .contains("failed to parse project config"));
        assert_eq!(
            std::fs::read_to_string(dir.path().join("owner.md")).unwrap(),
            original
        );
        assert_eq!(
            index.get_computed_fields("owner.md").unwrap()["derived"]
                .diagnostic
                .as_ref()
                .unwrap()
                .code,
            "module_error"
        );
    }

    #[test]
    fn changed_dependency_aborts_every_owner_write() {
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
        for (path, source) in [
            ("owner.md", "---\nname: owner\nderived: stale\n---\nOwner\n"),
            ("dependency.md", "---\nvalue: 1\n---\nDependency\n"),
        ] {
            std::fs::write(dir.path().join(path), source).unwrap();
            let parsed = crate::parser::parse_markdown_file(dir.path(), Path::new(path)).unwrap();
            index.upsert(&parsed, &[], &[]).unwrap();
        }
        index
            .replace_computed_fields(
                "owner.md",
                HashMap::from([(
                    "derived".to_string(),
                    ComputedFieldEntry {
                        module: "dependency_guard".to_string(),
                        definition_fingerprint: "old".to_string(),
                        input_fingerprint: None,
                        dependency_snapshot: ComputedDependencySnapshot::default(),
                        value_json: Some("\"stale\"".to_string()),
                        materialized_value_json: Some("\"stale\"".to_string()),
                        diagnostic: None,
                    },
                )]),
            )
            .unwrap();
        index.save().unwrap();

        let report = ModuleRunner::new(vec![Box::new(DependencyGuardModule)])
            .run_one(
                "dependency_guard",
                dir.path(),
                &index,
                &ModuleEvent::ManualRun { scope: None },
            )
            .unwrap();
        assert_eq!(report.fields_updated, 0);
        assert_eq!(report.diagnostics[0].code, "dependency_changed");
        let owner = crate::parser::parse_markdown_file(dir.path(), Path::new("owner.md"))
            .unwrap()
            .frontmatter
            .unwrap();
        assert!(owner.get("derived").is_none());
        assert_eq!(
            index.get_computed_fields("owner.md").unwrap()["derived"]
                .diagnostic
                .as_ref()
                .unwrap()
                .code,
            "dependency_changed"
        );
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
        std::fs::write(
            dir.path().join("invoice.md"),
            "---\ntotal: 12.3\nother: kept\n---\nBody\n",
        )
        .unwrap();
        let parsed =
            crate::parser::parse_markdown_file(dir.path(), Path::new("invoice.md")).unwrap();
        index.upsert(&parsed, &[], &[]).unwrap();

        let mut original = HashMap::new();
        original.insert(
            "total".to_string(),
            ComputedFieldEntry {
                module: "failing".to_string(),
                definition_fingerprint: "old".to_string(),
                input_fingerprint: None,
                dependency_snapshot: ComputedDependencySnapshot::default(),
                value_json: Some("12.3".to_string()),
                materialized_value_json: Some("12.3".to_string()),
                diagnostic: None,
            },
        );
        original.insert(
            "other".to_string(),
            ComputedFieldEntry {
                module: "other".to_string(),
                definition_fingerprint: "stable".to_string(),
                input_fingerprint: None,
                dependency_snapshot: ComputedDependencySnapshot::default(),
                value_json: Some("\"kept\"".to_string()),
                materialized_value_json: Some("\"kept\"".to_string()),
                diagnostic: None,
            },
        );
        index
            .replace_computed_fields("invoice.md", original)
            .unwrap();
        index.save().unwrap();

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
        let rewritten = crate::parser::parse_markdown_file(dir.path(), Path::new("invoice.md"))
            .unwrap()
            .frontmatter
            .unwrap();
        assert!(rewritten.get("total").is_none());
        assert_eq!(rewritten["other"], "kept");
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
                        input_fingerprint: None,
                        dependency_snapshot: ComputedDependencySnapshot::default(),
                        value_json: Some("4".to_string()),
                        materialized_value_json: Some("4".to_string()),
                        diagnostic: None,
                    },
                )]),
            )
            .unwrap();
        index.save().unwrap();

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

    #[test]
    fn invalid_schema_never_claims_or_removes_an_ordinary_schema_key() {
        let dir = TempDir::new().unwrap();
        std::fs::write(
            dir.path().join("note.md"),
            "---\n__schema__: user-value\n---\nBody\n",
        )
        .unwrap();
        std::fs::write(
            dir.path().join(".markdownvdb.schema.yml"),
            "fields:\n  broken:\n    field_type: formula\n    formula: '1'\n",
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
        let parsed = crate::parser::parse_markdown_file(dir.path(), Path::new("note.md")).unwrap();
        index.upsert(&parsed, &[], &[]).unwrap();
        index.save().unwrap();

        let report = ModuleRunner::builtins()
            .run_one(
                FORMULA_MODULE_ID,
                dir.path(),
                &index,
                &ModuleEvent::FullIngest,
            )
            .unwrap();
        assert_eq!(report.diagnostics[0].code, "invalid_schema");
        assert!(index.get_computed_fields("note.md").unwrap().is_empty());

        std::fs::remove_file(dir.path().join(".markdownvdb.schema.yml")).unwrap();
        ModuleRunner::builtins()
            .run_one(
                FORMULA_MODULE_ID,
                dir.path(),
                &index,
                &ModuleEvent::FullIngest,
            )
            .unwrap();
        let frontmatter = crate::parser::parse_markdown_file(dir.path(), Path::new("note.md"))
            .unwrap()
            .frontmatter
            .unwrap();
        assert_eq!(frontmatter["__schema__"], "user-value");
    }

    #[test]
    fn released_formula_tombstone_preserves_a_reclaimed_ordinary_key() {
        let dir = TempDir::new().unwrap();
        let schema_path = dir.path().join(".markdownvdb.schema.yml");
        let note_path = dir.path().join("note.md");
        std::fs::write(
            &schema_path,
            "fields:\n  legacy:\n    field_type: formula\n    formula: base * 2\n    result_type: number\n",
        )
        .unwrap();
        std::fs::write(&note_path, "---\nbase: 1\n---\nBody\n").unwrap();
        let index = Index::create(
            &dir.path().join("index.bin"),
            &EmbeddingConfig {
                provider: "test".to_string(),
                model: "test".to_string(),
                dimensions: 2,
            },
        )
        .unwrap();
        let parsed = crate::parser::parse_markdown_file(dir.path(), Path::new("note.md")).unwrap();
        index.upsert(&parsed, &[], &[]).unwrap();
        index.save().unwrap();

        let runner = ModuleRunner::builtins();
        runner
            .run_one(
                FORMULA_MODULE_ID,
                dir.path(),
                &index,
                &ModuleEvent::FullIngest,
            )
            .unwrap();
        assert_eq!(
            crate::parser::parse_markdown_file(dir.path(), Path::new("note.md"))
                .unwrap()
                .frontmatter
                .unwrap()["legacy"],
            JsonValue::from(2)
        );

        // Invalidating the overlay clears the old materialization but retains a
        // status tombstone which no longer owns the source key.
        std::fs::write(
            &schema_path,
            "fields:\n  broken:\n    field_type: formula\n    formula: '1'\n",
        )
        .unwrap();
        runner
            .run_one(
                FORMULA_MODULE_ID,
                dir.path(),
                &index,
                &ModuleEvent::FullIngest,
            )
            .unwrap();
        let tombstone = &index.get_computed_fields("note.md").unwrap()["legacy"];
        assert!(!tombstone.has_materialized_proof());

        // The user is now free to reclaim the old name as ordinary data. A new
        // Formula may consume it, and cleanup must not hide or delete it.
        std::fs::write(&note_path, "---\nbase: 1\nlegacy: 7\n---\nBody\n").unwrap();
        let reclaimed =
            crate::parser::parse_markdown_file(dir.path(), Path::new("note.md")).unwrap();
        index.refresh_source_metadata(&reclaimed).unwrap();
        index.save().unwrap();
        std::fs::write(
            &schema_path,
            "fields:\n  copied:\n    field_type: formula\n    formula: legacy + 1\n    result_type: number\n",
        )
        .unwrap();
        let report = runner
            .run_one(
                FORMULA_MODULE_ID,
                dir.path(),
                &index,
                &ModuleEvent::FullIngest,
            )
            .unwrap();
        assert!(report.diagnostics.is_empty(), "{:?}", report.diagnostics);

        let frontmatter = crate::parser::parse_markdown_file(dir.path(), Path::new("note.md"))
            .unwrap()
            .frontmatter
            .unwrap();
        assert_eq!(frontmatter["legacy"], JsonValue::from(7));
        assert_eq!(frontmatter["copied"], JsonValue::from(8));
        let fields = index.get_computed_fields("note.md").unwrap();
        assert!(!fields.contains_key("legacy"));
        assert_eq!(fields["copied"].value_json.as_deref(), Some("8"));
    }

    #[test]
    fn equal_ordinary_value_is_never_claimed_by_a_noop_materialization() {
        let dir = TempDir::new().unwrap();
        std::fs::write(
            dir.path().join(".markdownvdb.schema.yml"),
            "fields:\n  total:\n    field_type: formula\n    formula: price * 2\n    result_type: number\n",
        )
        .unwrap();
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
        index.save().unwrap();
        let original = std::fs::read(dir.path().join("invoice.md")).unwrap();

        ModuleRunner::builtins()
            .run_one(
                FORMULA_MODULE_ID,
                dir.path(),
                &index,
                &ModuleEvent::ManualRun { scope: None },
            )
            .unwrap();
        assert_eq!(
            std::fs::read(dir.path().join("invoice.md")).unwrap(),
            original
        );
        assert!(index.get_computed_fields("invoice.md").unwrap()["total"]
            .materialized_value_json
            .is_none());

        std::fs::write(dir.path().join(".markdownvdb.schema.yml"), "fields: {}\n").unwrap();
        ModuleRunner::builtins()
            .run_one(
                FORMULA_MODULE_ID,
                dir.path(),
                &index,
                &ModuleEvent::SchemaChanged,
            )
            .unwrap();
        assert_eq!(
            std::fs::read(dir.path().join("invoice.md")).unwrap(),
            original
        );
        let frontmatter = crate::parser::parse_markdown_file(dir.path(), Path::new("invoice.md"))
            .unwrap()
            .frontmatter
            .unwrap();
        assert_eq!(frontmatter["total"], JsonValue::from(4));
    }
}
