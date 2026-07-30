use std::fs;
use std::process::{Command, Output};
use std::sync::{Arc, Barrier};

use tempfile::TempDir;

fn mdvdb_bin() -> Command {
    Command::new(env!("CARGO_BIN_EXE_mdvdb"))
}

fn run(root: &std::path::Path, args: &[&str]) -> Output {
    mdvdb_bin()
        .args(args)
        .current_dir(root)
        .output()
        .expect("failed to execute mdvdb")
}

fn assert_success(output: &Output) {
    assert!(
        output.status.success(),
        "command failed\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

fn json(output: &Output) -> serde_json::Value {
    assert_success(output);
    serde_json::from_slice(&output.stdout).expect("stdout should be JSON")
}

fn setup_indexed_collection() -> TempDir {
    let dir = TempDir::new().unwrap();
    let root = dir.path();
    fs::create_dir_all(root.join(".markdownvdb")).unwrap();
    fs::create_dir_all(root.join("docs/api")).unwrap();
    fs::create_dir_all(root.join("docs-old")).unwrap();
    fs::write(
        root.join(".markdownvdb/config.yaml"),
        "embedding:\n  provider: mock\n  dimensions: 8\nunknown-key:\n  keep: true\n",
    )
    .unwrap();
    fs::write(
        root.join("docs/guide.md"),
        "---\ntitle: Guide\nkind: docs\n---\n# Guide\nshared scoped phrase\n\n[Legacy](../docs-old/legacy.md)\n",
    )
    .unwrap();
    fs::write(
        root.join("docs/api/auth.md"),
        "---\ntitle: Auth\nkind: docs\n---\n# Auth\nshared scoped phrase\n",
    )
    .unwrap();
    fs::write(
        root.join("docs-old/legacy.md"),
        "---\ntitle: Legacy\nkind: old\n---\n# Legacy\nshared scoped phrase\n",
    )
    .unwrap();

    assert_success(&run(root, &["ingest", "--json"]));
    assert_success(&run(
        root,
        &[
            "shards", "add", "docs", "--name", "Docs", "--path", "docs", "--json",
        ],
    ));
    dir
}

#[test]
fn shard_crud_is_json_stable_and_never_deletes_content() {
    let dir = TempDir::new().unwrap();
    let root = dir.path();
    fs::create_dir_all(root.join(".markdownvdb")).unwrap();
    fs::create_dir_all(root.join("work/research/papers")).unwrap();
    fs::write(root.join("work/research/note.md"), "# Keep me\n").unwrap();
    fs::write(
        root.join(".markdownvdb/config.yaml"),
        "unknown:\n  preserved: yes\n",
    )
    .unwrap();

    let added = json(&run(
        root,
        &[
            "shards",
            "create",
            "research",
            "--path",
            "work/research",
            "--name",
            "Research",
            "--json",
        ],
    ));
    assert_eq!(added["action"], "add");
    assert_eq!(added["shards"][0]["id"], "research");
    assert_eq!(added["shards"][0]["exists"], true);

    let child = json(&run(
        root,
        &[
            "shards",
            "add",
            "papers",
            "--path",
            "work/research/papers",
            "--json",
        ],
    ));
    assert_eq!(child["shards"][0]["name"], "Papers");

    let list = json(&run(root, &["shards", "list", "--json"]));
    assert_eq!(list["total_shards"], 2);
    let papers = list["shards"]
        .as_array()
        .unwrap()
        .iter()
        .find(|item| item["id"] == "papers")
        .unwrap();
    assert_eq!(papers["parent_id"], "research");

    let got = json(&run(root, &["shards", "get", "research", "--json"]));
    assert_eq!(got["path"], "work/research");

    let updated = json(&run(
        root,
        &[
            "shards",
            "update",
            "research",
            "--name",
            "Research Library",
            "--json",
        ],
    ));
    assert_eq!(updated["action"], "update");
    assert_eq!(updated["shards"][0]["name"], "Research Library");

    let removed = json(&run(root, &["shards", "delete", "research", "--json"]));
    assert_eq!(removed["action"], "remove");
    assert!(root.join("work/research/note.md").exists());

    let yaml = fs::read_to_string(root.join(".markdownvdb/config.yaml")).unwrap();
    assert!(yaml.contains("unknown:"));
    assert!(yaml.contains("preserved: yes"));
    assert!(!root.join(".markdownvdb/index").exists());
}

#[test]
fn shard_create_dir_retarget_and_missing_state_work_without_an_index() {
    let dir = TempDir::new().unwrap();
    let root = dir.path();

    let created = json(&run(
        root,
        &[
            "shards",
            "add",
            "drafts",
            "--path",
            "work/drafts",
            "--create-dir",
            "--json",
        ],
    ));
    assert_eq!(created["shards"][0]["exists"], true);
    assert!(root.join("work/drafts").is_dir());

    fs::create_dir_all(root.join("archive/drafts")).unwrap();
    let retargeted = json(&run(
        root,
        &[
            "shards",
            "retarget",
            "work/drafts",
            "archive/drafts",
            "--json",
        ],
    ));
    assert_eq!(retargeted["action"], "retarget");
    assert_eq!(retargeted["shards"][0]["path"], "archive/drafts");

    fs::remove_dir_all(root.join("archive/drafts")).unwrap();
    let missing = json(&run(root, &["shards", "get", "drafts", "--json"]));
    assert_eq!(missing["exists"], false);
}

#[test]
fn shard_scoped_commands_match_their_path_forms() {
    let dir = setup_indexed_collection();
    let root = dir.path();

    let cases: &[(&[&str], &[&str])] = &[
        (
            &["search", "shared scoped phrase", "--path", "docs", "--json"],
            &[
                "search",
                "shared scoped phrase",
                "--shard",
                "docs",
                "--json",
            ],
        ),
        (
            &["tree", "--path", "docs", "--json"],
            &["tree", "--shard", "docs", "--json"],
        ),
        (
            &["info", "docs", "--json"],
            &["info", "--shard", "docs", "--json"],
        ),
        (
            &["schema", "--path", "docs", "--json"],
            &["schema", "--shard", "docs", "--json"],
        ),
        (
            &["collection", "docs", "--recursive", "--json"],
            &["collection", "--shard", "docs", "--recursive", "--json"],
        ),
        (
            &["modules", "status", "formula", "--path", "docs", "--json"],
            &["modules", "status", "formula", "--shard", "docs", "--json"],
        ),
    ];

    for (path_args, shard_args) in cases {
        let mut path_json = json(&run(root, path_args));
        let mut shard_json = json(&run(root, shard_args));
        if path_args.first() == Some(&"search") {
            for value in [&mut path_json, &mut shard_json] {
                value["results"]
                    .as_array_mut()
                    .unwrap()
                    .sort_by(|left, right| {
                        left["file"]["path"]
                            .as_str()
                            .cmp(&right["file"]["path"].as_str())
                    });
            }
        }
        assert_eq!(
            path_json, shard_json,
            "{shard_args:?} should be equivalent to {path_args:?}"
        );
    }

    let tree = json(&run(root, &["tree", "--shard", "docs", "--json"]));
    assert_eq!(tree["total_files"], 2);
    let search = json(&run(
        root,
        &[
            "search",
            "shared scoped phrase",
            "--shard",
            "docs",
            "--limit",
            "10",
            "--json",
        ],
    ));
    assert!(search["results"]
        .as_array()
        .unwrap()
        .iter()
        .all(|item| item["file"]["path"].as_str().unwrap().starts_with("docs/")));
}

#[test]
fn shard_scoped_module_run_matches_its_path_form() {
    let dir = setup_indexed_collection();
    let root = dir.path();

    let path_report = json(&run(
        root,
        &["modules", "run", "formula", "--path", "docs", "--json"],
    ));
    let shard_report = json(&run(
        root,
        &["modules", "run", "formula", "--shard", "docs", "--json"],
    ));

    // Runtime duration is intentionally nondeterministic; all semantic report
    // fields must match because --shard resolves to the same path scope.
    for field in [
        "module",
        "event",
        "files_evaluated",
        "fields_updated",
        "diagnostics",
    ] {
        assert_eq!(
            path_report[field], shard_report[field],
            "module report field {field} should be path/Shard equivalent"
        );
    }
}

#[test]
fn cross_shard_links_stay_global_while_the_scoped_graph_is_closed() {
    let dir = setup_indexed_collection();
    let root = dir.path();

    let links = json(&run(root, &["links", "docs/guide.md", "--json"]));
    assert!(links["links"]["outgoing"]
        .as_array()
        .unwrap()
        .iter()
        .any(|link| link["entry"]["target"] == "docs-old/legacy.md"));

    let backlinks = json(&run(root, &["backlinks", "docs-old/legacy.md", "--json"]));
    assert!(backlinks["backlinks"]
        .as_array()
        .unwrap()
        .iter()
        .any(|link| link["entry"]["source"] == "docs/guide.md"));

    let graph = json(&run(root, &["graph", "--shard", "docs", "--json"]));
    let node_ids: std::collections::HashSet<&str> = graph["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|node| node["id"].as_str())
        .collect();
    assert!(graph["edges"].as_array().unwrap().iter().all(|edge| {
        edge["source"]
            .as_str()
            .is_some_and(|source| node_ids.contains(source))
            && edge["target"]
                .as_str()
                .is_some_and(|target| node_ids.contains(target))
    }));
    assert!(!node_ids.contains("docs-old/legacy.md"));

    let search = json(&run(
        root,
        &[
            "search",
            "shared scoped phrase",
            "--shard",
            "docs",
            "--expand",
            "1",
            "--limit",
            "10",
            "--json",
        ],
    ));
    assert!(search["results"].as_array().unwrap().iter().all(|result| {
        result["file"]["path"]
            .as_str()
            .is_some_and(|path| path.starts_with("docs/"))
    }));
    assert!(search["graph_context"]
        .as_array()
        .unwrap()
        .iter()
        .any(|context| context["file"]["path"] == "docs-old/legacy.md"));
}

#[test]
fn path_and_shard_conflict_and_unknown_ids_are_agent_friendly() {
    let dir = TempDir::new().unwrap();
    let conflict = run(
        dir.path(),
        &[
            "search", "query", "--path", "docs", "--shard", "docs", "--json",
        ],
    );
    assert!(!conflict.status.success());
    assert!(String::from_utf8_lossy(&conflict.stderr).contains("cannot be used with"));

    let unknown = run(dir.path(), &["tree", "--shard", "does-not-exist", "--json"]);
    assert!(!unknown.status.success());
    assert!(String::from_utf8_lossy(&unknown.stderr).contains("does-not-exist"));

    for args in [
        vec!["status", "--shard", "docs"],
        vec!["doctor", "--shard", "docs"],
        vec!["get", "docs/note.md", "--shard", "docs"],
    ] {
        let unsupported = run(dir.path(), &args);
        assert!(
            !unsupported.status.success(),
            "{args:?} must not acquire Shard semantics"
        );
        assert!(
            String::from_utf8_lossy(&unsupported.stderr).contains("unexpected argument"),
            "{args:?} should fail at argument parsing"
        );
    }
}

#[test]
fn shard_cluster_crud_and_graph_visibility_are_agent_friendly() {
    let dir = setup_indexed_collection();
    let root = dir.path();

    let added = json(&run(
        root,
        &[
            "clusters",
            "--shard",
            "docs",
            "add",
            "Engineering",
            "--description",
            "Engineering documents",
            "--seeds",
            "systems,design",
            "--json",
        ],
    ));
    assert_eq!(added["action"], "add");
    assert_eq!(added["shard_id"], "docs");
    assert_eq!(added["topics"][0]["name"], "Engineering");

    let listed = json(&run(
        root,
        &["clusters", "list", "--shard", "docs", "--json"],
    ));
    assert_eq!(listed.as_array().unwrap().len(), 1);
    assert_eq!(listed[0]["name"], "Engineering");

    // Ingest is the only operation allowed to create/recreate local Topic
    // centroids. Computed reads then use the sidecar without provider calls.
    assert_success(&run(root, &["ingest", "--json"]));
    let custom = json(&run(
        root,
        &["clusters", "--shard", "docs", "--custom", "--json"],
    ));
    assert!(custom.is_array());
    let automatic = json(&run(root, &["clusters", "--shard", "docs", "--json"]));
    assert!(automatic.is_array());
    let unassigned = json(&run(
        root,
        &["clusters", "--shard", "docs", "unassigned", "--json"],
    ));
    assert!(unassigned["count"].is_number());
    assert!(unassigned["paths"].is_array());

    let descendant = json(&run(
        root,
        &["graph", "--shard", "docs", "--path", "docs/api", "--json"],
    ));
    assert_eq!(descendant["nodes"].as_array().unwrap().len(), 1);
    assert_eq!(descendant["analysis"]["context"], "shard");
    assert_eq!(descendant["analysis"]["shard_id"], "docs");

    let ancestor = json(&run(
        root,
        &["graph", "--shard", "docs", "--path", ".", "--json"],
    ));
    assert_eq!(ancestor["nodes"].as_array().unwrap().len(), 2);
    let disjoint = json(&run(
        root,
        &["graph", "--shard", "docs", "--path", "docs-old", "--json"],
    ));
    assert!(disjoint["nodes"].as_array().unwrap().is_empty());
    assert!(disjoint["clusters"].as_array().unwrap().is_empty());

    let updated = json(&run(
        root,
        &[
            "clusters",
            "--shard",
            "docs",
            "update",
            "Engineering",
            "--rename",
            "Architecture",
            "--description",
            "Architecture documents",
            "--json",
        ],
    ));
    assert_eq!(updated["action"], "update");
    assert_eq!(updated["topics"][0]["name"], "Architecture");
    let removed = json(&run(
        root,
        &[
            "clusters",
            "--shard",
            "docs",
            "remove",
            "Architecture",
            "--json",
        ],
    ));
    assert_eq!(removed["action"], "remove");
    assert!(removed["topics"].as_array().unwrap().is_empty());
}

#[test]
fn shard_management_does_not_change_index_bytes_or_status() {
    let dir = setup_indexed_collection();
    let root = dir.path();
    let index_path = root.join(".markdownvdb/index");
    let before_bytes = fs::read(&index_path).unwrap();
    let before_status = json(&run(root, &["status", "--json"]));

    assert_success(&run(
        root,
        &[
            "shards",
            "update",
            "docs",
            "--name",
            "Documentation",
            "--json",
        ],
    ));
    assert_success(&run(root, &["shards", "remove", "docs", "--json"]));

    assert_eq!(fs::read(index_path).unwrap(), before_bytes);
    assert_eq!(json(&run(root, &["status", "--json"])), before_status);
}

#[test]
fn malformed_shards_do_not_break_unrelated_config_loading() {
    let dir = TempDir::new().unwrap();
    let root = dir.path();
    fs::create_dir_all(root.join(".markdownvdb")).unwrap();
    fs::write(
        root.join(".markdownvdb/config.yaml"),
        "embedding:\n  provider: mock\n  dimensions: 8\nshards:\n  - not-a-map\n",
    )
    .unwrap();

    assert_success(&run(root, &["status", "--json"]));
    let shard_list = run(root, &["shards", "list", "--json"]);
    assert!(!shard_list.status.success());
    assert!(String::from_utf8_lossy(&shard_list.stderr).contains("shard"));
}

#[test]
fn user_level_shards_never_leak_into_project_manifests() {
    let project = TempDir::new().unwrap();
    let user_config = TempDir::new().unwrap();
    fs::write(
        user_config.path().join("config.yaml"),
        "shards:\n  global:\n    name: Global\n    path: docs\n",
    )
    .unwrap();
    fs::create_dir_all(project.path().join("docs")).unwrap();

    let output = mdvdb_bin()
        .args(["shards", "list", "--json"])
        .current_dir(project.path())
        .env("MDVDB_CONFIG_HOME", user_config.path())
        .output()
        .expect("failed to execute mdvdb");
    let list = json(&output);
    assert_eq!(list["total_shards"], 0);
    assert_eq!(list["shards"], serde_json::json!([]));
}

#[test]
fn help_and_all_shell_completions_advertise_shards() {
    let help = run(std::path::Path::new("."), &["--help"]);
    assert_success(&help);
    assert!(String::from_utf8_lossy(&help.stdout).contains("shards"));

    for shell in ["bash", "zsh", "fish", "power-shell"] {
        let output = run(std::path::Path::new("."), &["completions", shell]);
        assert_success(&output);
        let text = String::from_utf8_lossy(&output.stdout);
        assert!(text.contains("shards"), "{shell} should advertise shards");
        assert!(
            shell == "power-shell" || text.contains("shard"),
            "{shell} should advertise --shard"
        );
        if shell != "power-shell" {
            let shard_flag = if shell == "fish" {
                "-l shard"
            } else {
                "--shard"
            };
            assert!(
                text.contains("clusters") && text.contains(shard_flag),
                "{shell} should advertise clusters --shard"
            );
        }
    }
}

#[test]
fn concurrent_topic_adds_do_not_lose_definitions() {
    let dir = TempDir::new().unwrap();
    let root = dir.path().to_path_buf();
    fs::create_dir_all(root.join(".markdownvdb")).unwrap();
    fs::write(
        root.join(".markdownvdb/config.yaml"),
        "embedding:\n  provider: mock\n  dimensions: 8\nunknown-key: retained\n",
    )
    .unwrap();

    let barrier = Arc::new(Barrier::new(8));
    let threads: Vec<_> = (0..8)
        .map(|index| {
            let root = root.clone();
            let barrier = Arc::clone(&barrier);
            std::thread::spawn(move || {
                barrier.wait();
                run(
                    &root,
                    &[
                        "clusters",
                        "add",
                        &format!("Topic {index}"),
                        "--description",
                        "Concurrent topic",
                        "--json",
                    ],
                )
            })
        })
        .collect();

    for thread in threads {
        assert_success(&thread.join().unwrap());
    }

    let list = json(&run(&root, &["clusters", "list", "--json"]));
    let definitions = list.as_array().unwrap();
    assert_eq!(definitions.len(), 8);
    for index in 0..8 {
        assert!(
            definitions
                .iter()
                .any(|definition| definition["name"] == format!("Topic {index}")),
            "Topic {index} should survive concurrent additions"
        );
    }

    let raw = fs::read_to_string(root.join(".markdownvdb/config.yaml")).unwrap();
    let yaml: serde_yaml::Value = serde_yaml::from_str(&raw).unwrap();
    assert_eq!(
        yaml["unknown-key"],
        serde_yaml::Value::String("retained".to_string())
    );
}
