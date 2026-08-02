//! Named, recursive folder lenses within a collection.
//!
//! Shards are project-local configuration only. They do not own an index,
//! change document identities, or create an access boundary. Definitions are
//! read directly from `<root>/.markdownvdb/config.yaml` so user-level
//! configuration can never leak Shards into a collection.

use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_yaml::{Mapping, Value};

use crate::clustering::CustomClusterDef;
use crate::config::{acquire_config_lock, write_yaml_value_unlocked};
use crate::error::Error;
use crate::Result;

const SHARDS_KEY: &str = "shards";
const NAME_KEY: &str = "name";
const PATH_KEY: &str = "path";
const TOPICS_KEY: &str = "topics";

/// Persisted Shard definition.
///
/// `id` is stored as the key beneath the top-level `shards` mapping. It is
/// immutable once added. `path` is always collection-root-relative and uses
/// `/` separators.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ShardDefinition {
    pub id: String,
    pub name: String,
    pub path: String,
}

/// A Shard definition enriched with derived hierarchy and filesystem state.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ShardInfo {
    pub id: String,
    pub name: String,
    pub path: String,
    pub parent_id: Option<String>,
    pub exists: bool,
}

/// Stable response wrapper used by Shard list APIs.
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
pub struct ShardList {
    pub shards: Vec<ShardInfo>,
    pub total_shards: usize,
}

/// Stable response wrapper used by Shard mutation APIs.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ShardMutation {
    pub action: String,
    pub shards: Vec<ShardInfo>,
}

/// Stable response wrapper used by Shard-local topic mutations.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ShardTopicMutation {
    pub action: String,
    pub shard_id: String,
    pub topics: Vec<CustomClusterDef>,
}

/// Project-local Shard manifest access.
#[derive(Debug, Clone)]
pub struct ShardStore {
    root: PathBuf,
    config_path: PathBuf,
}

impl ShardStore {
    /// Open the Shard manifest for a collection root.
    ///
    /// Construction performs no I/O and does not require an index or an
    /// existing `.markdownvdb` directory.
    pub fn new(root: impl Into<PathBuf>) -> Self {
        let root = root.into();
        let config_path = root.join(".markdownvdb").join("config.yaml");
        Self { root, config_path }
    }

    /// The raw project YAML used by this store.
    pub fn config_path(&self) -> &Path {
        &self.config_path
    }

    /// List all definitions in deterministic hierarchical path order.
    pub fn list(&self) -> Result<ShardList> {
        let root = read_raw_config(&self.config_path)?;
        let definitions = parse_definitions(&root)?;
        Ok(make_list(&self.root, &definitions))
    }

    /// Get one Shard by its immutable ID.
    pub fn get(&self, id: &str) -> Result<ShardInfo> {
        let list = self.list()?;
        list.shards
            .into_iter()
            .find(|shard| shard.id == id)
            .ok_or_else(|| shard_error(format!("Shard '{id}' was not found")))
    }

    /// Resolve a Shard ID to its canonical path for a path-scoped operation.
    ///
    /// Missing definitions remain listable and editable, but cannot be used as
    /// a command scope until their directory is restored or retargeted.
    pub fn resolve_path(&self, id: &str) -> Result<String> {
        let shard = self.get(id)?;
        if !shard.exists {
            return Err(shard_error(format!(
                "Shard '{}' points to missing folder '{}'",
                shard.id, shard.path
            )));
        }
        Ok(shard.path)
    }

    /// Read topic definitions owned by one Shard.
    ///
    /// Topic YAML is intentionally parsed separately from the Shard
    /// definition. A malformed `topics` value therefore fails only topic and
    /// analysis operations for that Shard; listing and editing Shards remains
    /// available for repair.
    pub fn topics(&self, id: &str) -> Result<Vec<CustomClusterDef>> {
        let root = read_raw_config(&self.config_path)?;
        let definitions = parse_definitions(&root)?;
        if !definitions.iter().any(|definition| definition.id == id) {
            return Err(shard_error(format!("Shard '{id}' was not found")));
        }
        parse_topics(&root, id)
    }

    /// Add a Shard-local topic definition.
    pub fn add_topic(&self, id: &str, definition: CustomClusterDef) -> Result<ShardTopicMutation> {
        self.mutate_topics(id, "add", |topics| {
            if topics.iter().any(|topic| topic.name == definition.name) {
                return Err(shard_error(format!(
                    "topic '{}' already exists in Shard '{id}'",
                    definition.name
                )));
            }
            topics.push(definition);
            Ok(())
        })
    }

    /// Update a Shard-local topic, keeping its list position and numeric ID.
    pub fn update_topic(
        &self,
        id: &str,
        name: &str,
        replacement: CustomClusterDef,
    ) -> Result<ShardTopicMutation> {
        self.mutate_topics(id, "update", |topics| {
            let Some(index) = topics.iter().position(|topic| topic.name == name) else {
                return Err(shard_error(format!(
                    "topic '{name}' was not found in Shard '{id}'"
                )));
            };
            if topics
                .iter()
                .enumerate()
                .any(|(other, topic)| other != index && topic.name == replacement.name)
            {
                return Err(shard_error(format!(
                    "topic '{}' already exists in Shard '{id}'",
                    replacement.name
                )));
            }
            topics[index] = replacement;
            Ok(())
        })
    }

    /// Remove a Shard-local topic definition.
    pub fn remove_topic(&self, id: &str, name: &str) -> Result<ShardTopicMutation> {
        self.mutate_topics(id, "remove", |topics| {
            let before = topics.len();
            topics.retain(|topic| topic.name != name);
            if topics.len() == before {
                return Err(shard_error(format!(
                    "topic '{name}' was not found in Shard '{id}'"
                )));
            }
            Ok(())
        })
    }

    fn mutate_topics(
        &self,
        id: &str,
        action: &str,
        mutate: impl FnOnce(&mut Vec<CustomClusterDef>) -> Result<()>,
    ) -> Result<ShardTopicMutation> {
        let _lock = acquire_config_lock(&self.config_path)?;
        let mut root = read_raw_config(&self.config_path)?;
        let definitions = parse_definitions(&root)?;
        if !definitions.iter().any(|definition| definition.id == id) {
            return Err(shard_error(format!("Shard '{id}' was not found")));
        }

        let mut topics = parse_topics(&root, id)?;
        let original_topics = topics.clone();
        let original_yaml = raw_topic_values(&root, id)?;
        mutate(&mut topics)?;
        topics = normalize_topic_definitions(topics)?;

        let shards = shards_mapping_mut(&mut root, false)?;
        let entry = shard_entry_mapping_mut(shards, id)?;
        entry.insert(
            string_value(TOPICS_KEY),
            Value::Sequence(merge_topic_yaml(
                &original_yaml,
                &original_topics,
                &topics,
                action,
            )),
        );
        write_yaml_value_unlocked(&self.config_path, &root)?;

        Ok(ShardTopicMutation {
            action: action.to_string(),
            shard_id: id.to_string(),
            topics,
        })
    }

    /// Add a new Shard definition.
    ///
    /// When `create_dir` is false the target must already be a directory.
    /// When true, a missing target and its parents are created.
    pub fn add(&self, definition: ShardDefinition, create_dir: bool) -> Result<ShardMutation> {
        let definition = normalize_definition(definition)?;
        let _lock = acquire_config_lock(&self.config_path)?;
        let mut root = read_raw_config(&self.config_path)?;
        let mut definitions = parse_definitions(&root)?;

        if definitions.iter().any(|item| item.id == definition.id) {
            return Err(shard_error(format!(
                "Shard '{}' already exists",
                definition.id
            )));
        }

        definitions.push(definition.clone());
        validate_definition_set(&definitions)?;
        ensure_target_directory(&self.root, &definition.path, create_dir)?;

        let shards = shards_mapping_mut(&mut root, true)?;
        let mut entry = Mapping::new();
        entry.insert(
            string_value(NAME_KEY),
            Value::String(definition.name.clone()),
        );
        entry.insert(
            string_value(PATH_KEY),
            Value::String(definition.path.clone()),
        );
        shards.insert(string_value(&definition.id), Value::Mapping(entry));
        write_yaml_value_unlocked(&self.config_path, &root)?;

        Ok(ShardMutation {
            action: "add".to_string(),
            shards: vec![make_info(&self.root, &definition, &definitions)],
        })
    }

    /// Update the editable fields of a Shard.
    ///
    /// The ID is deliberately not editable. Supplying `create_dir` without a
    /// new path creates the currently configured missing folder.
    pub fn update(
        &self,
        id: &str,
        name: Option<String>,
        path: Option<String>,
        create_dir: bool,
    ) -> Result<ShardMutation> {
        if name.is_none() && path.is_none() && !create_dir {
            return Err(shard_error(
                "Shard update requires --name, --path, or --create-dir",
            ));
        }

        let _lock = acquire_config_lock(&self.config_path)?;
        let mut root = read_raw_config(&self.config_path)?;
        let mut definitions = parse_definitions(&root)?;
        let Some(index) = definitions.iter().position(|item| item.id == id) else {
            return Err(shard_error(format!("Shard '{id}' was not found")));
        };

        if let Some(name) = name {
            definitions[index].name = normalize_name(&name)?;
        }
        let path_was_changed = path.is_some();
        if let Some(path) = path {
            definitions[index].path = normalize_shard_path(&path)?;
        }

        validate_definition_set(&definitions)?;
        if path_was_changed || create_dir {
            ensure_target_directory(&self.root, &definitions[index].path, create_dir)?;
        }

        let updated = definitions[index].clone();
        let shards = shards_mapping_mut(&mut root, false)?;
        let entry = shard_entry_mapping_mut(shards, id)?;
        entry.insert(string_value(NAME_KEY), Value::String(updated.name.clone()));
        entry.insert(string_value(PATH_KEY), Value::String(updated.path.clone()));
        write_yaml_value_unlocked(&self.config_path, &root)?;
        if path_was_changed {
            remove_analysis_cache_best_effort(&self.root, id);
        }

        Ok(ShardMutation {
            action: "update".to_string(),
            shards: vec![make_info(&self.root, &updated, &definitions)],
        })
    }

    /// Remove only a Shard definition. Its folder and contents are untouched.
    pub fn remove(&self, id: &str) -> Result<ShardMutation> {
        let _lock = acquire_config_lock(&self.config_path)?;
        let mut root = read_raw_config(&self.config_path)?;
        let definitions = parse_definitions(&root)?;
        let Some(definition) = definitions.iter().find(|item| item.id == id).cloned() else {
            return Err(shard_error(format!("Shard '{id}' was not found")));
        };
        let removed_info = make_info(&self.root, &definition, &definitions);

        let shards = shards_mapping_mut(&mut root, false)?;
        shards.remove(string_value(id));
        write_yaml_value_unlocked(&self.config_path, &root)?;
        remove_analysis_cache_best_effort(&self.root, id);

        Ok(ShardMutation {
            action: "remove".to_string(),
            shards: vec![removed_info],
        })
    }

    /// Atomically retarget every Shard at or beneath an old folder prefix.
    ///
    /// The new base directory must already exist. Descendant definitions may
    /// remain missing, which is important when repairing a partially moved
    /// folder hierarchy.
    pub fn retarget(&self, old_prefix: &str, new_prefix: &str) -> Result<ShardMutation> {
        let old_prefix = normalize_shard_path(old_prefix)?;
        let new_prefix = normalize_shard_path(new_prefix)?;
        if old_prefix == new_prefix {
            return Err(shard_error("old and new Shard prefixes are identical"));
        }

        let _lock = acquire_config_lock(&self.config_path)?;
        let mut root = read_raw_config(&self.config_path)?;
        let mut definitions = parse_definitions(&root)?;

        let mut affected_ids = Vec::new();
        for definition in &mut definitions {
            if let Some(relative) = relative_to_scope(&definition.path, &old_prefix) {
                definition.path = if relative.is_empty() {
                    new_prefix.clone()
                } else {
                    format!("{new_prefix}/{relative}")
                };
                affected_ids.push(definition.id.clone());
            }
        }
        if affected_ids.is_empty() {
            return Err(shard_error(format!(
                "no Shards exist at or below '{old_prefix}'"
            )));
        }

        validate_definition_set(&definitions)?;
        ensure_target_directory(&self.root, &new_prefix, false)?;

        let shards = shards_mapping_mut(&mut root, false)?;
        for definition in &definitions {
            if affected_ids.iter().any(|id| id == &definition.id) {
                let entry = shard_entry_mapping_mut(shards, &definition.id)?;
                entry.insert(
                    string_value(PATH_KEY),
                    Value::String(definition.path.clone()),
                );
            }
        }
        write_yaml_value_unlocked(&self.config_path, &root)?;
        for id in &affected_ids {
            remove_analysis_cache_best_effort(&self.root, id);
        }

        let affected: HashSet<&str> = affected_ids.iter().map(String::as_str).collect();
        let mut infos: Vec<_> = definitions
            .iter()
            .filter(|definition| affected.contains(definition.id.as_str()))
            .map(|definition| make_info(&self.root, definition, &definitions))
            .collect();
        sort_infos(&mut infos);

        Ok(ShardMutation {
            action: "retarget".to_string(),
            shards: infos,
        })
    }
}

fn read_raw_config(path: &Path) -> Result<Value> {
    if !path.exists() {
        return Ok(Value::Mapping(Mapping::new()));
    }

    let content = fs::read_to_string(path).map_err(|e| {
        Error::Config(format!(
            "failed to read project config '{}': {e}",
            path.display()
        ))
    })?;
    if content.trim().is_empty() {
        return Ok(Value::Mapping(Mapping::new()));
    }

    let value: Value = serde_yaml::from_str(&content).map_err(|e| {
        shard_error(format!(
            "failed to parse project config '{}': {e}",
            path.display()
        ))
    })?;
    if !value.is_mapping() {
        return Err(shard_error(format!(
            "project config '{}' must be a YAML mapping",
            path.display()
        )));
    }
    Ok(value)
}

fn parse_definitions(root: &Value) -> Result<Vec<ShardDefinition>> {
    let root = root
        .as_mapping()
        .ok_or_else(|| shard_error("project config must be a YAML mapping"))?;
    let Some(shards) = root.get(string_value(SHARDS_KEY)) else {
        return Ok(Vec::new());
    };
    let shards = shards
        .as_mapping()
        .ok_or_else(|| shard_error("top-level 'shards' value must be a mapping"))?;

    let mut definitions = Vec::with_capacity(shards.len());
    for (id, entry) in shards {
        let id = id
            .as_str()
            .ok_or_else(|| shard_error("every Shard ID must be a string"))?;
        let entry = entry
            .as_mapping()
            .ok_or_else(|| shard_error(format!("Shard '{id}' must be a mapping")))?;
        let name = required_string(entry, id, NAME_KEY)?;
        let path = required_string(entry, id, PATH_KEY)?;
        definitions.push(normalize_definition(ShardDefinition {
            id: id.to_string(),
            name,
            path,
        })?);
    }

    validate_definition_set(&definitions)?;
    Ok(definitions)
}

fn parse_topics(root: &Value, id: &str) -> Result<Vec<CustomClusterDef>> {
    let root = root
        .as_mapping()
        .ok_or_else(|| shard_error("project config must be a YAML mapping"))?;
    let shards = root
        .get(string_value(SHARDS_KEY))
        .and_then(Value::as_mapping)
        .ok_or_else(|| shard_error("top-level 'shards' value must be a mapping"))?;
    let entry = shards
        .get(string_value(id))
        .and_then(Value::as_mapping)
        .ok_or_else(|| shard_error(format!("Shard '{id}' must be a mapping")))?;
    let Some(value) = entry.get(string_value(TOPICS_KEY)) else {
        return Ok(Vec::new());
    };
    let topics =
        serde_yaml::from_value::<Vec<CustomClusterDef>>(value.clone()).map_err(|error| {
            shard_error(format!("Shard '{id}' has malformed local topics: {error}"))
        })?;
    normalize_topic_definitions(topics)
}

fn raw_topic_values(root: &Value, id: &str) -> Result<Vec<Value>> {
    let root = root
        .as_mapping()
        .ok_or_else(|| shard_error("project config must be a YAML mapping"))?;
    let shards = root
        .get(string_value(SHARDS_KEY))
        .and_then(Value::as_mapping)
        .ok_or_else(|| shard_error("top-level 'shards' value must be a mapping"))?;
    let entry = shards
        .get(string_value(id))
        .and_then(Value::as_mapping)
        .ok_or_else(|| shard_error(format!("Shard '{id}' must be a mapping")))?;
    match entry.get(string_value(TOPICS_KEY)) {
        None => Ok(Vec::new()),
        Some(Value::Sequence(topics)) => Ok(topics.clone()),
        Some(_) => Err(shard_error(format!(
            "Shard '{id}' has malformed local topics: expected a sequence"
        ))),
    }
}

fn merge_topic_yaml(
    original_yaml: &[Value],
    original_topics: &[CustomClusterDef],
    topics: &[CustomClusterDef],
    action: &str,
) -> Vec<Value> {
    topics
        .iter()
        .enumerate()
        .map(|(index, topic)| {
            // Updates preserve list position, including a renamed Topic's
            // unknown extension keys. Adds/removes match unchanged entries by
            // their unique name.
            let original_index = if action == "update" && original_yaml.len() == topics.len() {
                Some(index)
            } else {
                original_topics
                    .iter()
                    .position(|original| original.name == topic.name)
            };
            let mut mapping = original_index
                .and_then(|index| original_yaml.get(index))
                .and_then(Value::as_mapping)
                .cloned()
                .unwrap_or_default();

            mapping.insert(string_value(NAME_KEY), Value::String(topic.name.clone()));
            set_optional_string(&mut mapping, "description", topic.description.as_deref());
            if topic.seeds.is_empty() {
                mapping.remove(string_value("seeds"));
            } else {
                mapping.insert(
                    string_value("seeds"),
                    Value::Sequence(
                        topic
                            .seeds
                            .iter()
                            .map(|seed| Value::String(seed.clone()))
                            .collect(),
                    ),
                );
            }
            if let Some(threshold) = topic.threshold {
                mapping.insert(
                    string_value("threshold"),
                    Value::Number(serde_yaml::Number::from(threshold as f64)),
                );
            } else {
                mapping.remove(string_value("threshold"));
            }
            Value::Mapping(mapping)
        })
        .collect()
}

fn set_optional_string(mapping: &mut Mapping, key: &str, value: Option<&str>) {
    if let Some(value) = value {
        mapping.insert(string_value(key), Value::String(value.to_string()));
    } else {
        mapping.remove(string_value(key));
    }
}

/// Validate and normalize topic definitions using the same constraints as
/// collection-level custom clusters.
pub fn normalize_topic_definitions(topics: Vec<CustomClusterDef>) -> Result<Vec<CustomClusterDef>> {
    let mut normalized = Vec::with_capacity(topics.len());
    let mut names = HashSet::new();
    for mut topic in topics {
        topic.name = topic.name.trim().to_string();
        topic.description = topic
            .description
            .map(|description| description.trim().to_string())
            .filter(|description| !description.is_empty());
        topic.seeds = topic
            .seeds
            .into_iter()
            .map(|seed| seed.trim().to_string())
            .filter(|seed| !seed.is_empty())
            .collect();

        if topic.name.is_empty() {
            return Err(shard_error("topic name cannot be empty"));
        }
        if !names.insert(topic.name.clone()) {
            return Err(shard_error(format!(
                "duplicate topic name '{}' within one Shard",
                topic.name
            )));
        }
        if topic.description.is_none() && topic.seeds.is_empty() {
            return Err(shard_error(format!(
                "topic '{}' needs a description or at least one seed",
                topic.name
            )));
        }
        if let Some(threshold) = topic.threshold {
            if !(0.0..=1.0).contains(&threshold) {
                return Err(shard_error(format!(
                    "topic '{}' threshold ({threshold}) must be in [0.0, 1.0]",
                    topic.name
                )));
            }
        }
        normalized.push(topic);
    }
    Ok(normalized)
}

fn remove_analysis_cache_best_effort(root: &Path, id: &str) {
    let _ = crate::shard_analysis::remove_cache(root, id);
}

fn required_string(entry: &Mapping, id: &str, key: &str) -> Result<String> {
    entry
        .get(string_value(key))
        .and_then(Value::as_str)
        .map(ToOwned::to_owned)
        .ok_or_else(|| shard_error(format!("Shard '{id}' requires a string '{key}' field")))
}

fn normalize_definition(definition: ShardDefinition) -> Result<ShardDefinition> {
    validate_id(&definition.id)?;
    Ok(ShardDefinition {
        id: definition.id,
        name: normalize_name(&definition.name)?,
        path: normalize_shard_path(&definition.path)?,
    })
}

fn validate_definition_set(definitions: &[ShardDefinition]) -> Result<()> {
    let mut ids = HashSet::new();
    let mut names: HashMap<String, &str> = HashMap::new();
    let mut paths: HashMap<&str, &str> = HashMap::new();

    for definition in definitions {
        validate_id(&definition.id)?;
        if !ids.insert(definition.id.as_str()) {
            return Err(shard_error(format!(
                "duplicate Shard ID '{}'",
                definition.id
            )));
        }

        let folded_name = definition.name.to_lowercase();
        if let Some(existing) = names.insert(folded_name, &definition.id) {
            return Err(shard_error(format!(
                "Shard names must be unique ignoring case ('{}' and '{}')",
                existing, definition.id
            )));
        }
        if let Some(existing) = paths.insert(&definition.path, &definition.id) {
            return Err(shard_error(format!(
                "Shard paths must be unique ('{}' and '{}')",
                existing, definition.id
            )));
        }
    }
    Ok(())
}

fn validate_id(id: &str) -> Result<()> {
    let valid = !id.is_empty()
        && !id.starts_with('-')
        && !id.ends_with('-')
        && id
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
        && !id.as_bytes().windows(2).any(|pair| pair == b"--");

    if !valid {
        return Err(shard_error(format!(
            "Shard ID '{id}' must be kebab-case (lowercase letters, digits, and single hyphens)"
        )));
    }
    Ok(())
}

fn normalize_name(name: &str) -> Result<String> {
    let name = name.trim();
    if name.is_empty() {
        return Err(shard_error("Shard name cannot be empty"));
    }
    Ok(name.to_string())
}

/// Normalize and validate a collection-root-relative Shard folder path.
pub fn normalize_shard_path(path: &str) -> Result<String> {
    let replaced = path.trim().replace('\\', "/");
    if replaced.starts_with('/') || is_windows_drive_path(&replaced) {
        return Err(shard_error(format!(
            "Shard path '{path}' must be collection-root-relative"
        )));
    }

    let mut components = Vec::new();
    for component in replaced.split('/') {
        match component {
            "" | "." => continue,
            ".." => {
                return Err(shard_error(format!(
                    "Shard path '{path}' cannot contain '..'"
                )));
            }
            value if value.eq_ignore_ascii_case(".markdownvdb") => {
                return Err(shard_error(
                    "Shard paths cannot target an internal .markdownvdb directory",
                ));
            }
            value if value.contains('\0') => {
                return Err(shard_error("Shard paths cannot contain NUL bytes"));
            }
            value => components.push(value),
        }
    }

    if components.is_empty() {
        return Err(shard_error(
            "Shard path cannot be empty or the collection root",
        ));
    }
    Ok(components.join("/"))
}

fn is_windows_drive_path(path: &str) -> bool {
    let bytes = path.as_bytes();
    bytes.len() >= 2 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':'
}

fn ensure_target_directory(root: &Path, relative: &str, create_dir: bool) -> Result<()> {
    let target = root.join(relative);
    if target.is_dir() {
        return Ok(());
    }
    if target.exists() {
        return Err(shard_error(format!(
            "Shard path '{}' is not a directory",
            relative
        )));
    }
    if !create_dir {
        return Err(shard_error(format!(
            "Shard folder '{}' does not exist (use --create-dir to create it)",
            relative
        )));
    }

    fs::create_dir_all(&target)
        .map_err(|e| shard_error(format!("failed to create Shard folder '{}': {e}", relative)))?;
    Ok(())
}

fn make_list(root: &Path, definitions: &[ShardDefinition]) -> ShardList {
    let mut shards: Vec<_> = definitions
        .iter()
        .map(|definition| make_info(root, definition, definitions))
        .collect();
    sort_infos(&mut shards);
    ShardList {
        total_shards: shards.len(),
        shards,
    }
}

fn make_info(
    root: &Path,
    definition: &ShardDefinition,
    definitions: &[ShardDefinition],
) -> ShardInfo {
    let parent_id = definitions
        .iter()
        .filter(|candidate| candidate.id != definition.id)
        .filter(|candidate| relative_to_scope(&definition.path, &candidate.path).is_some())
        .max_by_key(|candidate| component_count(&candidate.path))
        .map(|candidate| candidate.id.clone());

    ShardInfo {
        id: definition.id.clone(),
        name: definition.name.clone(),
        path: definition.path.clone(),
        parent_id,
        exists: root.join(&definition.path).is_dir(),
    }
}

fn sort_infos(infos: &mut [ShardInfo]) {
    infos.sort_by(|left, right| {
        compare_component_paths(&left.path, &right.path)
            .then_with(|| left.name.to_lowercase().cmp(&right.name.to_lowercase()))
            .then_with(|| left.id.cmp(&right.id))
    });
}

fn compare_component_paths(left: &str, right: &str) -> std::cmp::Ordering {
    let mut left = left.split('/');
    let mut right = right.split('/');
    loop {
        match (left.next(), right.next()) {
            (Some(left), Some(right)) => match left.cmp(right) {
                std::cmp::Ordering::Equal => continue,
                ordering => return ordering,
            },
            (None, Some(_)) => return std::cmp::Ordering::Less,
            (Some(_), None) => return std::cmp::Ordering::Greater,
            (None, None) => return std::cmp::Ordering::Equal,
        }
    }
}

fn component_count(path: &str) -> usize {
    path.split('/').count()
}

/// Return `path` relative to `scope` when it is equal to or a segment-safe
/// descendant of `scope`.
fn relative_to_scope<'a>(path: &'a str, scope: &str) -> Option<&'a str> {
    if !crate::path_util::path_is_in_scope(path, scope) {
        return None;
    }
    if path == scope {
        return Some("");
    }
    path.strip_prefix(scope)?.strip_prefix('/')
}

fn shards_mapping_mut(root: &mut Value, create: bool) -> Result<&mut Mapping> {
    let root = root
        .as_mapping_mut()
        .ok_or_else(|| shard_error("project config must be a YAML mapping"))?;
    let key = string_value(SHARDS_KEY);
    if !root.contains_key(&key) {
        if !create {
            return Err(shard_error("project has no Shard definitions"));
        }
        root.insert(key.clone(), Value::Mapping(Mapping::new()));
    }
    root.get_mut(&key)
        .and_then(Value::as_mapping_mut)
        .ok_or_else(|| shard_error("top-level 'shards' value must be a mapping"))
}

fn shard_entry_mapping_mut<'a>(shards: &'a mut Mapping, id: &str) -> Result<&'a mut Mapping> {
    shards
        .get_mut(string_value(id))
        .and_then(Value::as_mapping_mut)
        .ok_or_else(|| shard_error(format!("Shard '{id}' must be a mapping")))
}

fn string_value(value: &str) -> Value {
    Value::String(value.to_string())
}

fn shard_error(message: impl Into<String>) -> Error {
    Error::Shard(message.into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_windows_and_redundant_separators() {
        assert_eq!(
            normalize_shard_path(r"./work\\research//papers/").unwrap(),
            "work/research/papers"
        );
    }

    #[test]
    fn scope_matching_is_segment_safe() {
        assert_eq!(relative_to_scope("docs", "docs"), Some(""));
        assert_eq!(relative_to_scope("docs/api", "docs"), Some("api"));
        assert_eq!(relative_to_scope("docs-old", "docs"), None);
    }

    #[test]
    fn component_sort_keeps_ancestors_before_descendants() {
        let mut paths = [
            "work/research/z",
            "work-archive",
            "work/research",
            "work",
            "work/research/a",
        ];
        paths.sort_by(|left, right| compare_component_paths(left, right));
        assert_eq!(
            paths,
            [
                "work",
                "work/research",
                "work/research/a",
                "work/research/z",
                "work-archive",
            ]
        );
    }
}
