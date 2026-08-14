use std::fs;
use std::path::PathBuf;

#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;

#[cfg(unix)]
use std::process::Command;

fn repository_file(path: &str) -> String {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    fs::read_to_string(root.join(path)).unwrap_or_else(|error| panic!("read {path}: {error}"))
}

#[test]
fn cli_release_smoke_builds_do_not_publish() {
    let workflow = repository_file(".github/workflows/release-cli.yml");

    assert!(workflow.contains("workflow_dispatch:"));
    assert!(workflow.contains("default: mac"));
    assert!(workflow.contains("if: github.event_name == 'push' && startsWith(github.ref"));
    assert!(workflow.contains("draft: true"));
    assert!(workflow.contains("needs: build"));
}

#[test]
fn macos_cli_is_verified_before_it_is_packaged() {
    let workflow = repository_file(".github/workflows/release-cli.yml");
    let signing = workflow.find("- name: Sign and verify macOS CLI").unwrap();
    let notarization = workflow.find("- name: Notarize and assess macOS CLI").unwrap();
    let packaging = workflow.find("- name: Package (Unix)").unwrap();

    assert!(signing < notarization);
    assert!(notarization < packaging);
    assert!(workflow.contains("--identifier dev.mdvdb.cli"));
    assert!(workflow.contains("Developer ID Application: Industrial Code & Magic GmbH"));
    assert!(workflow.contains("scripts/notarize-cli-macos.sh"));
}

#[test]
fn notarization_is_observable_and_bounded() {
    let script = repository_file("scripts/notarize-cli-macos.sh");

    assert!(script.starts_with("#!/usr/bin/env bash\nset -euo pipefail"));
    assert!(script.contains("notarytool submit"));
    assert!(script.contains("--no-wait"));
    assert!(script.contains("notarytool info"));
    assert!(script.contains("Apple notarization submitted:"));
    assert!(script.contains("NOTARIZATION_TIMEOUT_MINUTES"));
    assert!(script.contains("spctl --assess --type execute"));
}

#[test]
fn release_asset_names_preserve_the_desktop_installer_contract() {
    let workflow = repository_file(".github/workflows/release-cli.yml");

    assert!(workflow.contains("raw_binary=\"mdvdb-${{ matrix.target }}\""));
    assert!(workflow.contains("$rawBinary = \"mdvdb-${{ matrix.target }}.exe\""));
}

#[cfg(unix)]
#[test]
fn notarization_script_completes_the_accepted_flow() {
    let temporary_directory = tempfile::tempdir().unwrap();
    let command_directory = temporary_directory.path().join("bin");
    fs::create_dir(&command_directory).unwrap();

    write_executable(
        command_directory.join("xcrun"),
        "#!/bin/sh\nexit 0\n",
    );
    write_executable(
        command_directory.join("plutil"),
        "#!/bin/sh\nif [ \"$2\" = id ]; then echo submission-id; else echo Accepted; fi\n",
    );
    write_executable(
        command_directory.join("codesign"),
        "#!/bin/sh\nexit 0\n",
    );
    write_executable(
        command_directory.join("spctl"),
        "#!/bin/sh\necho 'accepted source=Notarized Developer ID'\n",
    );

    let archive = temporary_directory.path().join("submission.zip");
    let binary = temporary_directory.path().join("mdvdb");
    fs::write(&archive, b"archive").unwrap();
    fs::write(&binary, b"binary").unwrap();

    let inherited_path = std::env::var("PATH").unwrap_or_default();
    let output = Command::new("bash")
        .arg(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("scripts/notarize-cli-macos.sh"))
        .arg(&archive)
        .arg(&binary)
        .env("PATH", format!("{}:{inherited_path}", command_directory.display()))
        .env("PLUTIL_BIN", command_directory.join("plutil"))
        .env("APPLE_ID", "developer@example.com")
        .env("APPLE_APP_SPECIFIC_PASSWORD", "test-password")
        .env("APPLE_TEAM_ID", "ABCDE12345")
        .env("NOTARIZATION_TIMEOUT_MINUTES", "1")
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "stdout: {}\nstderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("Apple notarization submitted: submission-id"));
    assert!(stdout.contains("Apple notarization accepted and Gatekeeper approved"));
}

#[cfg(unix)]
fn write_executable(path: PathBuf, contents: &str) {
    fs::write(&path, contents).unwrap();
    let mut permissions = fs::metadata(&path).unwrap().permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(path, permissions).unwrap();
}
