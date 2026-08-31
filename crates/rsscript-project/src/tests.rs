    use super::*;

    #[test]
    fn project_manifest_capture_is_bounded_and_preserves_the_parser_input() {
        let directory = tempfile::tempdir().expect("workspace");
        std::fs::write(
            directory.path().join("rsspkg.toml"),
            "[package]\nname = \"fixture\"\nversion = \"0.1.0\"\nedition = \"2024\"\n",
        )
        .expect("manifest");
        let snapshot = capture_project_manifest(directory.path(), 1024).expect("capture manifest");
        assert_eq!(
            snapshot.root(),
            directory
                .path()
                .canonicalize()
                .expect("canonical workspace root")
        );
        assert!(snapshot.source().contains("name = \"fixture\""));
        assert!(capture_project_manifest(directory.path(), 4).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn project_manifest_capture_rejects_a_symlink() {
        use std::os::unix::fs::symlink;

        let directory = tempfile::tempdir().expect("workspace");
        let outside = directory.path().join("outside.toml");
        std::fs::write(&outside, "[package]\nname = \"outside\"\n").expect("outside manifest");
        symlink(&outside, directory.path().join("rsspkg.toml")).expect("manifest link");
        assert!(capture_project_manifest(directory.path(), 1024).is_err());
    }

    #[test]
    fn project_boundary_resolves_only_present_local_dependency_roots() {
        let directory = tempfile::tempdir().expect("workspace");
        let package = directory.path().join("package");
        let dependency = directory.path().join("dependency");
        std::fs::create_dir_all(&package).expect("package root");
        std::fs::create_dir_all(&dependency).expect("dependency root");
        std::fs::write(package.join("rsspkg.toml"), "[package]\nname = \"root\"\n")
            .expect("package manifest");
        std::fs::write(
            dependency.join("rsspkg.toml"),
            "[package]\nname = \"dependency\"\n",
        )
        .expect("dependency manifest");

        assert_eq!(
            resolve_project_path_dependency(&package, "../dependency")
                .expect("resolve present dependency"),
            Some(
                package
                    .canonicalize()
                    .expect("canonical package")
                    .join("../dependency")
            )
        );
        assert_eq!(
            resolve_project_path_dependency(&package, "../missing")
                .expect("missing dependency is an unresolved package"),
            None
        );
    }

    #[test]
    fn manifest_graph_captures_local_dependencies_once_and_ignores_remote_forms() {
        let directory = tempfile::tempdir().expect("workspace");
        let root = directory.path().join("root");
        let dependency = directory.path().join("dependency");
        std::fs::create_dir_all(&root).expect("root");
        std::fs::create_dir_all(&dependency).expect("dependency");
        std::fs::write(
            root.join("rsspkg.toml"),
            "[package]\nname = \"root\"\n\n[dependencies]\nlocal = { path = \"../dependency\" }\nregistry = \"1.0\"\nremote = { git = \"https://example.invalid/repo\" }\n",
        )
        .expect("root manifest");
        std::fs::write(
            dependency.join("rsspkg.toml"),
            "[package]\nname = \"dependency\"\n\n[dependencies]\nroot = { path = \"../root\" }\n",
        )
        .expect("dependency manifest");

        let graph = capture_project_manifest_graph(
            &root,
            ProjectManifestGraphLimits {
                max_packages: 2,
                max_manifest_bytes: 1024,
                max_total_manifest_bytes: 2048,
            },
        )
        .expect("bounded manifest graph");
        assert_eq!(graph.root(), root.canonicalize().as_deref().expect("root"));
        assert_eq!(graph.packages().len(), 2);
        let root_package = graph.package(&root).expect("root package");
        assert_eq!(root_package.path_dependencies().len(), 1);
        assert_eq!(root_package.path_dependencies()[0].name(), "local");
        assert_eq!(
            root_package.path_dependencies()[0].section(),
            WorkspaceDependencySection::Dependencies
        );
        assert!(graph.package(&dependency).is_some());
    }

    #[test]
    fn manifest_graph_enforces_total_manifest_budget() {
        let directory = tempfile::tempdir().expect("workspace");
        std::fs::write(
            directory.path().join("rsspkg.toml"),
            "[package]\nname = \"root\"\n",
        )
        .expect("manifest");
        let error = capture_project_manifest_graph(
            directory.path(),
            ProjectManifestGraphLimits {
                max_packages: 1,
                max_manifest_bytes: 1024,
                max_total_manifest_bytes: 1,
            },
        )
        .expect_err("aggregate manifest budget must be enforced");
        assert!(error.contains("total manifest byte limit"), "{error}");
    }

    #[test]
    fn optional_project_utf8_returns_none_for_missing_and_reads_confined_file() {
        let directory = tempfile::tempdir().expect("workspace");
        assert_eq!(
            capture_optional_project_utf8(directory.path(), "identity", 1024, "identity")
                .expect("missing optional file"),
            None
        );
        std::fs::write(directory.path().join("identity"), "captured").expect("identity file");
        assert_eq!(
            capture_optional_project_utf8(directory.path(), "identity", 1024, "identity")
                .expect("captured optional file"),
            Some("captured".to_string())
        );
    }

    #[test]
    fn project_source_capture_is_confined_bounded_and_excludes_manifest_roots() {
        let directory = tempfile::tempdir().expect("workspace");
        std::fs::create_dir_all(directory.path().join("src/ignored")).expect("source tree");
        std::fs::write(
            directory.path().join("src/main.rss"),
            "fn main() -> Unit { return Unit }\n",
        )
        .expect("source");
        std::fs::write(directory.path().join("src/api.rssi"), "module api\n").expect("interface");
        std::fs::write(directory.path().join("src/ignored/hidden.rss"), "hidden")
            .expect("excluded source");
        std::fs::write(directory.path().join("src/readme.txt"), "ignored").expect("non-source");

        let mut capture = ProjectSourceCapture::new(
            directory.path(),
            ProjectSourceCaptureLimits {
                max_files: 2,
                max_total_bytes: 1024,
                max_file_bytes: 1024,
                max_depth: 8,
            },
        )
        .expect("capture boundary");
        let files = capture
            .capture(&["src".to_string()], &["src/ignored".to_string()])
            .expect("bounded capture");
        assert_eq!(
            files
                .iter()
                .map(|file| file.relative_path())
                .collect::<Vec<_>>(),
            vec!["src/api.rssi", "src/main.rss"]
        );
        assert!(capture.capture(&["../outside".to_string()], &[]).is_err());
    }

    #[test]
    fn project_tree_scan_is_bounded_sorted_and_policy_filtered() {
        let directory = tempfile::tempdir().expect("workspace");
        std::fs::create_dir_all(directory.path().join("nested")).expect("tree");
        std::fs::write(directory.path().join("z.txt"), "z").expect("source");
        std::fs::write(directory.path().join("a.txt"), "a").expect("source");
        std::fs::write(directory.path().join("nested/keep.txt"), "keep").expect("source");
        std::fs::write(directory.path().join("nested/skip.txt"), "skip").expect("source");

        let files = collect_project_regular_files(
            directory.path(),
            ProjectTreeLimits {
                max_files: 3,
                max_entries: 8,
                max_bytes: 32,
                max_depth: 4,
            },
            "project scan test",
            |_, name| name == "skip.txt",
        )
        .expect("bounded tree scan");
        assert_eq!(
            files
                .iter()
                .map(|file| {
                    file.path
                        .strip_prefix(directory.path())
                        .expect("project-relative output")
                        .display()
                        .to_string()
                })
                .collect::<Vec<_>>(),
            vec!["a.txt", "nested/keep.txt", "z.txt"]
        );
        let error = collect_project_regular_files(
            directory.path(),
            ProjectTreeLimits {
                max_files: 2,
                max_entries: 8,
                max_bytes: 32,
                max_depth: 4,
            },
            "project scan test",
            |_, name| name == "skip.txt",
        )
        .expect_err("file limit must apply before a fourth accepted file");
        assert!(error.contains("file count limit"), "{error}");
    }

    #[test]
    fn project_graph_capture_is_private_bounded_and_maps_paths_back() {
        let directory = tempfile::tempdir().expect("workspace");
        let package = directory.path().join("package");
        std::fs::create_dir_all(package.join("nested")).expect("package directories");
        std::fs::write(
            package.join("main.rss"),
            "fn main() -> Unit { return Unit }",
        )
        .expect("source");
        std::fs::write(package.join("nested/input.txt"), "captured").expect("input");
        std::fs::write(package.join("target"), "excluded").expect("excluded input");

        let graph = capture_project_graph([package.clone()], ["target"], None)
            .expect("bounded graph capture");
        let captured = graph
            .captured_path(&package)
            .expect("original package has a captured path");
        assert_eq!(
            std::fs::read_to_string(captured.join("nested/input.txt")).expect("captured contents"),
            "captured"
        );
        assert!(!captured.join("target").exists());
        assert_eq!(
            graph.original_path(&captured.join("nested/input.txt")),
            Some(package.join("nested/input.txt"))
        );
        assert_eq!(
            graph
                .read_captured_utf8(&package, Path::new("nested/input.txt"), 1024)
                .expect("read captured text"),
            "captured"
        );
        graph
            .replace_captured_utf8(
                &package,
                Path::new("nested/input.txt"),
                "captured",
                "rewritten",
                1024,
            )
            .expect("rewrite private capture");
        assert_eq!(
            graph
                .read_captured_utf8(&package, Path::new("nested/input.txt"), 1024)
                .expect("read rewritten text"),
            "rewritten"
        );
        assert!(
            graph
                .replace_captured_utf8(
                    &package,
                    Path::new("nested/input.txt"),
                    "captured",
                    "ignored",
                    1024,
                )
                .is_err()
        );
        graph
            .create_captured_utf8(
                &package,
                Path::new("capture-metadata.toml"),
                "identity = 'captured'\n",
                1024,
            )
            .expect("create capture-owned metadata");
        assert_eq!(
            graph
                .read_captured_utf8(&package, Path::new("capture-metadata.toml"), 1024)
                .expect("read capture metadata"),
            "identity = 'captured'\n"
        );
        assert!(
            graph
                .create_captured_utf8(
                    &package,
                    Path::new("capture-metadata.toml"),
                    "replacement",
                    1024,
                )
                .is_err()
        );
        assert!(
            graph
                .read_captured_utf8(&package, Path::new("../outside"), 1024)
                .is_err()
        );
        let captured_input = captured.join("nested/input.txt");
        let selected = graph
            .select_package_root(&package)
            .expect("select captured package root");
        assert!(selected.root().is_dir());
        assert_eq!(
            selected.original_path(&captured_input),
            Some(package.join("nested/input.txt"))
        );
        assert_eq!(
            selected.remap_path_label(&captured_input.display().to_string()),
            package.join("nested/input.txt").display().to_string()
        );
        assert!(
            selected
                .remap_error(format!("failed under {}", selected.root().display()))
                .contains(&package.display().to_string())
        );
    }

    #[test]
    fn project_graph_capture_reuses_parent_snapshot_for_nested_roots() {
        let directory = tempfile::tempdir().expect("workspace");
        let root = directory.path().join("root");
        let dependency = root.join("deps/helper");
        std::fs::create_dir_all(dependency.join("interface")).expect("dependency directories");
        std::fs::write(root.join("rsspkg.toml"), "[package]\nname = 'root'\n")
            .expect("root manifest");
        std::fs::write(
            dependency.join("rsspkg.toml"),
            "[package]\nname = 'helper'\n",
        )
        .expect("dependency manifest");
        std::fs::write(
            dependency.join("interface/lib.rssi"),
            "pub fn Helper.value() -> Int\n",
        )
        .expect("dependency interface");

        let graph = capture_project_graph(
            [dependency.clone(), root.clone()],
            std::iter::empty::<&str>(),
            None,
        )
        .expect("nested package roots should share one private capture");
        let captured_root = graph.captured_path(&root).expect("root mapping");
        let captured_dependency = graph
            .captured_path(&dependency)
            .expect("dependency mapping");
        assert_eq!(captured_dependency, captured_root.join("deps/helper"));
        assert_eq!(
            graph
                .read_captured_utf8(&dependency, Path::new("interface/lib.rssi"), 1024)
                .expect("nested dependency contents"),
            "pub fn Helper.value() -> Int\n"
        );
        assert_eq!(
            graph.original_path(&captured_dependency.join("interface/lib.rssi")),
            Some(dependency.join("interface/lib.rssi"))
        );
    }

    #[cfg(unix)]
    #[test]
    fn project_graph_capture_rejects_links_without_reading_their_target() {
        use std::os::unix::fs::symlink;

        let directory = tempfile::tempdir().expect("workspace");
        let package = directory.path().join("package");
        let outside = directory.path().join("outside");
        std::fs::create_dir_all(&package).expect("package directory");
        std::fs::write(&outside, "outside").expect("outside input");
        symlink(&outside, package.join("link")).expect("fixture link");

        let error = capture_project_graph([package], std::iter::empty::<&str>(), None)
            .expect_err("links are not a valid package capture input");
        assert!(error.contains("rejects symlinks"), "{error}");
        assert_eq!(
            std::fs::read_to_string(outside).expect("outside contents"),
            "outside"
        );
    }

    #[test]
    fn frontend_digest_excludes_test_files_but_retains_logical_source_identity() {
        let directory = tempfile::tempdir().expect("workspace");
        std::fs::create_dir_all(directory.path().join("tests")).expect("tests directory");
        std::fs::write(
            directory.path().join("main.rss"),
            "fn main() -> Unit { return Unit }\n",
        )
        .expect("source");
        std::fs::write(
            directory.path().join("tests/check.rss"),
            "fn check() -> Unit { return Unit }\n",
        )
        .expect("test");

        let project = ProjectLoader::new()
            .capture_from(directory.path(), Path::new("."))
            .expect("capture");
        assert!(project.content_digest().starts_with("sha256:"));
        assert!(project.frontend_digest().starts_with("sha256:"));
        assert!(
            project
                .files()
                .iter()
                .any(|file| file.kind == WorkspaceFileKind::Test)
        );
        assert_eq!(project.frontend().sources().files().len(), 1);
        assert_eq!(
            project.frontend().sources().files()[0].path(),
            "root/main.rss"
        );
    }

    #[test]
    fn captured_package_input_projects_only_compiler_frontend_bytes() {
        let input = PackageLoweringInput {
            package: PackageIdentity {
                name: "fixture".into(),
                version: "0.1.0".into(),
                edition: "2024".into(),
            },
            package_dir: "/host-specific/fixture".into(),
            source_path: "/host-specific/fixture/src/main.rss".into(),
            source_relative_path: "src/main.rss".into(),
            source: "fn main() -> Unit { return Unit }".into(),
            sources: vec![(
                "root/src/main.rss".into(),
                "fn main() -> Unit { return Unit }".into(),
            )],
            interfaces: vec![(
                "dep/api.rssi".into(),
                "module api\npub fn log(message: read String) -> Unit".into(),
            )],
            native_dependencies: vec![NativeRustDependency {
                crate_name: "fixture-native".into(),
                path: "/host-specific/native".into(),
                cargo_features: vec!["fast".into()],
                default_features: false,
                bindings: BTreeMap::new(),
            }],
        };
        let frontend = input.frontend_input();
        assert_eq!(frontend.sources().files().len(), 1);
        assert_eq!(frontend.interfaces().files().len(), 1);
        assert_eq!(frontend.sources().files()[0].path(), "root/src/main.rss");
        assert_eq!(frontend.interfaces().files()[0].path(), "dep/api.rssi");
    }
