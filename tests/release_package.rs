//===----------------------------------------------------------------------===//
// SPDX-License-Identifier: Apache-2.0 WITH LLVM-exception
//===----------------------------------------------------------------------===//
//! Exercise the real packager against a small resource tree and build fixture.
#![cfg(unix)]

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::process::Command;

fn checked(command: &mut Command) {
    let output = command.output().expect("run packaging command");
    assert!(
        output.status.success(),
        "{command:?}: {}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

fn fixture(root: &Path) {
    for relative in [
        "target/release/brievc",
        "lib/std/prelude.bv",
        "lib/runtime/briev_rt.c",
        "config/targets.toml",
        "config/nested/profile.toml",
        "config/.resource",
        "plugins/prelude.bv",
        "examples/hello.bv",
        "examples/counter.rbv",
        "LICENSE",
        "NOTICE",
        "README.md",
        "CHANGELOG.md",
        "RELEASE_NOTES.md",
    ] {
        let path = root.join(relative);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, relative).unwrap();
    }
    fs::create_dir_all(root.join("scripts")).unwrap();
    fs::copy(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("scripts/package_release.sh"),
        root.join("scripts/package_release.sh"),
    )
    .unwrap();
    fs::create_dir_all(root.join("bin")).unwrap();
    let cargo = root.join("bin/cargo");
    fs::write(
        &cargo,
        "#!/bin/sh\n[ \"$*\" = 'build --release --bin brievc' ]\n",
    )
    .unwrap();
    fs::set_permissions(cargo, fs::Permissions::from_mode(0o755)).unwrap();
}

#[test]
fn release_archive_resources_are_direct_and_repeatable() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    fixture(root);
    let path = std::env::join_paths(
        std::iter::once(root.join("bin"))
            .chain(std::env::split_paths(&std::env::var_os("PATH").unwrap())),
    )
    .unwrap();
    let mut package = Command::new("bash");
    package.current_dir(root).env("PATH", path).args([
        "scripts/package_release.sh",
        "v0.0.0-test",
        "artifact output",
    ]);
    checked(&mut package);
    let output = root.join("artifact output");
    let archive = output.join("briev-v0.0.0-test.tar.gz");
    let first = fs::read(&archive).unwrap();
    fs::write(output.join("briev-v0.0.0-test/config/stale"), "stale").unwrap();
    checked(&mut package);
    assert_eq!(first, fs::read(&archive).unwrap(), "repeatable archive");
    checked(
        Command::new("sha256sum")
            .current_dir(&output)
            .args(["--check", "briev-v0.0.0-test.tar.gz.sha256"]),
    );
    let extracted = root.join("extracted");
    fs::create_dir(&extracted).unwrap();
    checked(
        Command::new("tar")
            .arg("-xf")
            .arg(archive)
            .arg("-C")
            .arg(&extracted),
    );
    let tree = extracted.join("briev-v0.0.0-test");
    for relative in [
        "config/targets.toml",
        "config/nested/profile.toml",
        "config/.resource",
        "lib/std/prelude.bv",
        "lib/runtime/briev_rt.c",
        "plugins/prelude.bv",
        "NOTICE",
    ] {
        assert_eq!(fs::read_to_string(tree.join(relative)).unwrap(), relative);
    }
    assert!(!tree.join("config/config").exists());
    assert!(!tree.join("config/stale").exists());
    assert_ne!(
        fs::metadata(tree.join("brievc"))
            .unwrap()
            .permissions()
            .mode()
            & 0o111,
        0
    );
}
