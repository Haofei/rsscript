#[cfg(test)]
mod resource_boundary_tests {
    use super::super::*;

    fn oversized_sparse_file(name: &str) -> std::path::PathBuf {
        let path = std::env::temp_dir().join(format!(
            "rsscript-vm-{name}-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let file = std::fs::File::create(&path).expect("test file should be created");
        file.set_len(rsscript_runtime::RUNTIME_READ_CEILING_BYTES as u64 + 1)
            .expect("sparse test file should be sized");
        path
    }

    #[test]
    fn vm_file_cursor_does_not_advance_after_oversized_read() {
        let path = oversized_sparse_file("file-limit");
        let mut state = VmFileState {
            path: path.to_string_lossy().into_owned(),
            mode: "read".to_string(),
            cursor: 0,
        };

        file_read_remaining(&mut state).expect_err("oversized VM read should fail");

        assert_eq!(state.cursor, 0);
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn vm_file_backed_intrinsics_reject_oversized_inputs_before_parsing() {
        let path = oversized_sparse_file("intrinsic-file-limit");
        let path_text = path.to_string_lossy();

        assert!(file_bytes_stream_value(&path_text, 4096).is_err());
        assert!(csv_rows_stream_value(&path_text).is_err());
        assert!(toml_parse_file_value(&path_text).is_err());

        let _ = std::fs::remove_file(path);
    }

    #[test]
    #[cfg(unix)]
    fn vm_process_request_timeout_is_not_blocked_by_stdin() {
        let request = VmProcessRequest {
            command: "sh".to_string(),
            args: vec!["-c".to_string(), "sleep 5".to_string()],
            cwd: None,
            stdin: Some("x".repeat(8 * 1024 * 1024)),
            env: Vec::new(),
            timeout_ms: 30,
            merge_stderr: true,
            output_cap_bytes: 0,
        };
        let started = std::time::Instant::now();

        let error = process_run_request(&request).expect_err("VM process should time out");

        assert!(error.display().contains("timed out"), "{}", error.display());
        assert!(
            started.elapsed() < std::time::Duration::from_secs(2),
            "VM timeout was blocked by stdin"
        );
    }

    #[test]
    #[cfg(unix)]
    fn vm_directory_listing_rejects_symlink_cycles() {
        use std::os::unix::fs::symlink;

        let root = std::env::temp_dir().join(format!(
            "rsscript-vm-directory-cycle-{}",
            std::process::id()
        ));
        std::fs::create_dir_all(root.join("child")).expect("directory should be created");
        symlink(&root, root.join("child").join("cycle")).expect("symlink should be created");

        let error = directory_list_files(&root).expect_err("VM traversal should reject symlinks");

        assert!(error.to_string().contains("symbolic link"));
        let _ = std::fs::remove_dir_all(root);
    }
}

