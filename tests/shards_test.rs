use std::fs;
use std::sync::{Arc, Barrier};

use mdvdb::{Config, CustomClusterDef, Error, ShardDefinition, ShardStore};
use serial_test::serial;
use tempfile::TempDir;

fn write_config(root: &std::path::Path, yaml: &str) {
    let config_dir = root.join(".markdownvdb");
    fs::create_dir_all(&config_dir).unwrap();
    fs::write(config_dir.join("config.yaml"), yaml).unwrap();
}

fn definition(id: &str, name: &str, path: &str) -> ShardDefinition {
    ShardDefinition {
        id: id.to_string(),
        name: name.to_string(),
        path: path.to_string(),
    }
}

#[test]
fn crud_preserves_unknown_yaml_and_never_deletes_content() {
    let temp = TempDir::new().unwrap();
    let root = temp.path();
    fs::create_dir_all(root.join("work/research/papers")).unwrap();
    fs::create_dir_all(root.join("work/research/labs")).unwrap();
    fs::write(root.join("work/research/note.md"), "# Keep me").unwrap();
    write_config(
        root,
        r#"
embedding:
  provider: mock
unknown-plugin:
  enabled: true
shards:
  research:
    name: Research
    path: work/research
    future-field: retained
"#,
    );

    let store = ShardStore::new(root);
    let added = store
        .add(
            definition("papers", "Papers", r"work\research\papers\\"),
            false,
        )
        .unwrap();
    assert_eq!(added.action, "add");
    assert_eq!(added.shards[0].path, "work/research/papers");
    assert_eq!(added.shards[0].parent_id.as_deref(), Some("research"));

    let updated = store
        .update(
            "papers",
            Some("Research Papers".to_string()),
            Some("work/research/labs".to_string()),
            false,
        )
        .unwrap();
    assert_eq!(updated.action, "update");
    assert_eq!(updated.shards[0].name, "Research Papers");
    assert_eq!(updated.shards[0].path, "work/research/labs");

    let removed = store.remove("research").unwrap();
    assert_eq!(removed.action, "remove");
    assert_eq!(removed.shards[0].id, "research");
    assert!(root.join("work/research/note.md").is_file());

    let raw = fs::read_to_string(store.config_path()).unwrap();
    let yaml: serde_yaml::Value = serde_yaml::from_str(&raw).unwrap();
    assert_eq!(
        yaml["unknown-plugin"]["enabled"],
        serde_yaml::Value::Bool(true)
    );
    // Removing a definition removes its entry, while other definitions and
    // unrelated settings remain.
    assert!(yaml["shards"]["research"].is_null());
    assert_eq!(
        yaml["shards"]["papers"]["name"],
        serde_yaml::Value::String("Research Papers".to_string())
    );
    assert_eq!(
        yaml["embedding"]["provider"],
        serde_yaml::Value::String("mock".to_string())
    );
}

#[test]
fn update_preserves_unknown_fields_inside_shard_entry() {
    let temp = TempDir::new().unwrap();
    fs::create_dir_all(temp.path().join("docs")).unwrap();
    write_config(
        temp.path(),
        r#"
shards:
  docs:
    name: Documents
    path: docs
    color: blue
"#,
    );

    ShardStore::new(temp.path())
        .update("docs", Some("Docs".to_string()), None, false)
        .unwrap();

    let raw = fs::read_to_string(temp.path().join(".markdownvdb/config.yaml")).unwrap();
    let yaml: serde_yaml::Value = serde_yaml::from_str(&raw).unwrap();
    assert_eq!(
        yaml["shards"]["docs"]["color"],
        serde_yaml::Value::String("blue".to_string())
    );
}

#[test]
fn list_is_hierarchical_deterministic_and_keeps_missing_definitions() {
    let temp = TempDir::new().unwrap();
    fs::create_dir_all(temp.path().join("work/research/papers")).unwrap();
    fs::create_dir_all(temp.path().join("work-archive")).unwrap();
    write_config(
        temp.path(),
        r#"
shards:
  papers:
    name: Papers
    path: work/research/papers
  archive:
    name: Archive
    path: work-archive
  research:
    name: Research
    path: work/research
  work:
    name: Work
    path: work
  missing:
    name: Missing
    path: work/research/missing
"#,
    );

    let list = ShardStore::new(temp.path()).list().unwrap();
    assert_eq!(list.total_shards, 5);
    assert_eq!(
        list.shards
            .iter()
            .map(|item| item.id.as_str())
            .collect::<Vec<_>>(),
        vec!["work", "research", "missing", "papers", "archive"]
    );
    assert_eq!(list.shards[1].parent_id.as_deref(), Some("work"));
    assert_eq!(list.shards[2].parent_id.as_deref(), Some("research"));
    assert!(!list.shards[2].exists);
    assert!(matches!(
        ShardStore::new(temp.path()).resolve_path("missing"),
        Err(Error::Shard(_))
    ));
}

#[test]
fn create_dir_supports_new_empty_and_ignored_folders() {
    let temp = TempDir::new().unwrap();
    fs::write(temp.path().join(".mdvdbignore"), "generated/\n").unwrap();
    let store = ShardStore::new(temp.path());

    store
        .add(definition("generated", "Generated", "generated/deep"), true)
        .unwrap();

    assert!(temp.path().join("generated/deep").is_dir());
    assert!(store.get("generated").unwrap().exists);
}

#[test]
fn update_create_dir_repairs_the_current_missing_path() {
    let temp = TempDir::new().unwrap();
    write_config(
        temp.path(),
        r#"
shards:
  missing:
    name: Missing
    path: restored/deep
"#,
    );
    let store = ShardStore::new(temp.path());
    assert!(!store.get("missing").unwrap().exists);

    let mutation = store.update("missing", None, None, true).unwrap();
    assert_eq!(mutation.shards[0].path, "restored/deep");
    assert!(mutation.shards[0].exists);
    assert!(temp.path().join("restored/deep").is_dir());
}

#[test]
fn validates_ids_names_paths_and_existing_directory() {
    let temp = TempDir::new().unwrap();
    fs::create_dir_all(temp.path().join("docs")).unwrap();
    fs::create_dir_all(temp.path().join("docs-old")).unwrap();
    fs::write(temp.path().join("not-a-folder"), "file").unwrap();
    let store = ShardStore::new(temp.path());
    store
        .add(definition("docs", "Documents", "docs"), false)
        .unwrap();

    for id in ["Upper", "-bad", "bad-", "bad--id", "with_space"] {
        assert!(
            store
                .add(definition(id, "Unique", "docs-old"), false)
                .is_err(),
            "{id} should be rejected"
        );
    }
    assert!(store
        .add(definition("same-name", "documents", "docs-old"), false)
        .is_err());
    assert!(store
        .add(definition("same-path", "Other", r"docs\\"), false)
        .is_err());

    for path in [
        "",
        ".",
        "/",
        "../docs",
        "x/../docs",
        ".markdownvdb",
        "C:\\docs",
        "C:docs",
        "C:",
    ] {
        assert!(
            store
                .add(definition("bad-path", "Bad Path", path), false)
                .is_err(),
            "{path:?} should be rejected"
        );
    }
    assert!(store
        .add(definition("missing", "Missing", "missing"), false)
        .is_err());
    assert!(store
        .add(
            definition("not-folder", "Not Folder", "not-a-folder"),
            false,
        )
        .is_err());
}

#[test]
fn retarget_updates_only_segment_safe_descendants_and_preserves_fields() {
    let temp = TempDir::new().unwrap();
    fs::create_dir_all(temp.path().join("moved/research/papers")).unwrap();
    fs::create_dir_all(temp.path().join("work-archive")).unwrap();
    write_config(
        temp.path(),
        r#"
shards:
  research:
    name: Research
    path: work/research
    icon: flask
  papers:
    name: Papers
    path: work/research/papers
  archive:
    name: Archive
    path: work-archive
"#,
    );

    let store = ShardStore::new(temp.path());
    let mutation = store.retarget("work", "moved").unwrap();
    assert_eq!(mutation.action, "retarget");
    assert_eq!(
        mutation
            .shards
            .iter()
            .map(|item| item.id.as_str())
            .collect::<Vec<_>>(),
        vec!["research", "papers"]
    );
    assert_eq!(store.get("research").unwrap().path, "moved/research");
    assert_eq!(store.get("papers").unwrap().path, "moved/research/papers");
    assert_eq!(store.get("archive").unwrap().path, "work-archive");

    let raw = fs::read_to_string(store.config_path()).unwrap();
    let yaml: serde_yaml::Value = serde_yaml::from_str(&raw).unwrap();
    assert_eq!(
        yaml["shards"]["research"]["icon"],
        serde_yaml::Value::String("flask".to_string())
    );
}

#[test]
fn retarget_rejects_collisions_without_partial_write() {
    let temp = TempDir::new().unwrap();
    fs::create_dir_all(temp.path().join("new")).unwrap();
    write_config(
        temp.path(),
        r#"
shards:
  old:
    name: Old
    path: old
  occupied:
    name: Occupied
    path: new
"#,
    );
    let before = fs::read_to_string(temp.path().join(".markdownvdb/config.yaml")).unwrap();

    assert!(ShardStore::new(temp.path()).retarget("old", "new").is_err());
    let after = fs::read_to_string(temp.path().join(".markdownvdb/config.yaml")).unwrap();
    assert_eq!(after, before);
}

#[test]
#[serial]
fn malformed_shards_do_not_break_ordinary_config_loading() {
    let temp = TempDir::new().unwrap();
    write_config(
        temp.path(),
        r#"
embedding:
  provider: mock
  dimensions: 8
shards:
  - this-is-not-a-map
"#,
    );

    std::env::set_var("MDVDB_NO_USER_CONFIG", "1");
    let ordinary = Config::load(temp.path());
    std::env::remove_var("MDVDB_NO_USER_CONFIG");
    assert!(ordinary.is_ok());
    assert!(matches!(
        ShardStore::new(temp.path()).list(),
        Err(Error::Shard(_))
    ));
}

#[test]
fn concurrent_mutations_share_one_advisory_config_lock() {
    let temp = TempDir::new().unwrap();
    for index in 0..8 {
        fs::create_dir_all(temp.path().join(format!("folder-{index}"))).unwrap();
    }
    write_config(
        temp.path(),
        "embedding:\n  provider: mock\nfuture-setting: retained\n",
    );

    let store = Arc::new(ShardStore::new(temp.path()));
    let barrier = Arc::new(Barrier::new(8));
    let threads: Vec<_> = (0..8)
        .map(|index| {
            let store = Arc::clone(&store);
            let barrier = Arc::clone(&barrier);
            std::thread::spawn(move || {
                barrier.wait();
                store.add(
                    definition(
                        &format!("shard-{index}"),
                        &format!("Shard {index}"),
                        &format!("folder-{index}"),
                    ),
                    false,
                )
            })
        })
        .collect();

    for thread in threads {
        thread.join().unwrap().unwrap();
    }
    let list = store.list().unwrap();
    assert_eq!(list.total_shards, 8);
    let raw = fs::read_to_string(store.config_path()).unwrap();
    let yaml: serde_yaml::Value = serde_yaml::from_str(&raw).unwrap();
    assert_eq!(
        yaml["future-setting"],
        serde_yaml::Value::String("retained".to_string())
    );
}

#[test]
fn topic_and_settings_transactions_share_config_lock() {
    let temp = TempDir::new().unwrap();
    write_config(temp.path(), "future-setting: retained\n");
    let config_path = Arc::new(temp.path().join(".markdownvdb/config.yaml"));
    let barrier = Arc::new(Barrier::new(12));

    let mut threads = Vec::new();
    for index in 0..6 {
        let config_path = Arc::clone(&config_path);
        let barrier = Arc::clone(&barrier);
        threads.push(std::thread::spawn(move || -> mdvdb::Result<()> {
            barrier.wait();
            mdvdb::config::mutate_yaml_config_value(&config_path, "clustering.custom", |current| {
                let mut topics = current
                    .and_then(serde_yaml::Value::as_sequence)
                    .cloned()
                    .unwrap_or_default();
                // Widen the read/modify race window. The shared lock must
                // remain held while this callback runs.
                std::thread::sleep(std::time::Duration::from_millis(5));
                topics.push(serde_yaml::Value::String(format!("topic-{index}")));
                Ok((serde_yaml::Value::Sequence(topics), ()))
            })
        }));
    }
    for index in 0..6 {
        let config_path = Arc::clone(&config_path);
        let barrier = Arc::clone(&barrier);
        threads.push(std::thread::spawn(move || -> mdvdb::Result<()> {
            barrier.wait();
            mdvdb::config::update_yaml_config_value(
                &config_path,
                &format!("agent.slot-{index}"),
                serde_yaml::Value::Number(index.into()),
            )
        }));
    }

    for thread in threads {
        thread.join().unwrap().unwrap();
    }

    let raw = fs::read_to_string(config_path.as_ref()).unwrap();
    let yaml: serde_yaml::Value = serde_yaml::from_str(&raw).unwrap();
    assert_eq!(yaml["clustering"]["custom"].as_sequence().unwrap().len(), 6);
    assert_eq!(yaml["agent"].as_mapping().unwrap().len(), 6);
    assert_eq!(
        yaml["future-setting"],
        serde_yaml::Value::String("retained".to_string())
    );
}

#[test]
fn local_topic_crud_preserves_shard_and_unknown_yaml() {
    let temp = TempDir::new().unwrap();
    fs::create_dir_all(temp.path().join("docs")).unwrap();
    write_config(
        temp.path(),
        r#"
future-setting: retained
shards:
  docs:
    name: Documents
    path: docs
    color: blue
    topics:
      - name: Rust
        description: Systems programming
        seeds: [ownership]
        threshold: 0.42
        future-topic-field: retained
"#,
    );
    let store = ShardStore::new(temp.path());
    let topic = CustomClusterDef {
        name: "Other".to_string(),
        description: Some("Another topic".to_string()),
        seeds: vec![],
        threshold: None,
    };

    let added = store.add_topic("docs", topic).unwrap();
    assert_eq!(added.action, "add");
    assert_eq!(store.topics("docs").unwrap()[0].name, "Rust");

    let replacement = CustomClusterDef {
        name: "Rust language".to_string(),
        description: None,
        seeds: vec!["borrow checker".to_string()],
        threshold: None,
    };
    let updated = store.update_topic("docs", "Rust", replacement).unwrap();
    assert_eq!(updated.topics[0].name, "Rust language");

    let raw = fs::read_to_string(store.config_path()).unwrap();
    let yaml: serde_yaml::Value = serde_yaml::from_str(&raw).unwrap();
    assert_eq!(yaml["future-setting"], "retained");
    assert_eq!(yaml["shards"]["docs"]["color"], "blue");
    assert_eq!(
        yaml["shards"]["docs"]["topics"][0]["future-topic-field"],
        "retained"
    );

    store.remove_topic("docs", "Rust language").unwrap();
    assert_eq!(store.topics("docs").unwrap()[0].name, "Other");
    assert!(temp.path().join("docs").is_dir());
}

#[test]
fn malformed_local_topics_do_not_break_shard_listing_or_other_shards() {
    let temp = TempDir::new().unwrap();
    fs::create_dir_all(temp.path().join("good")).unwrap();
    fs::create_dir_all(temp.path().join("bad")).unwrap();
    write_config(
        temp.path(),
        r#"
shards:
  good:
    name: Good
    path: good
    topics:
      - name: Valid
        seeds: [one]
  bad:
    name: Bad
    path: bad
    topics: definitely-not-a-list
"#,
    );
    let store = ShardStore::new(temp.path());

    assert_eq!(store.list().unwrap().total_shards, 2);
    assert_eq!(store.topics("good").unwrap()[0].name, "Valid");
    assert!(matches!(store.topics("bad"), Err(Error::Shard(_))));
}

#[test]
fn local_topic_names_are_unique_only_within_each_shard() {
    let temp = TempDir::new().unwrap();
    fs::create_dir_all(temp.path().join("one")).unwrap();
    fs::create_dir_all(temp.path().join("two")).unwrap();
    let store = ShardStore::new(temp.path());
    store.add(definition("one", "One", "one"), false).unwrap();
    store.add(definition("two", "Two", "two"), false).unwrap();
    let topic = || CustomClusterDef {
        name: "Shared".to_string(),
        description: None,
        seeds: vec!["seed".to_string()],
        threshold: None,
    };

    store.add_topic("one", topic()).unwrap();
    store.add_topic("two", topic()).unwrap();
    assert!(store.add_topic("one", topic()).is_err());
}
