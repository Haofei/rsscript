#[cfg(test)]
mod resource_boundary_tests {
    use super::super::*;

    #[test]
    fn vm_file_cursor_does_not_advance_without_provider() {
        let mut state = VmFileState {
            path: "data.bin".to_string(),
            mode: "read".to_string(),
            cursor: 0,
        };
        let error = file_read_remaining(&mut state).expect_err("provider-less read should fail");
        assert!(error.to_string().contains("external provider"));
        assert_eq!(state.cursor, 0);
    }

    #[test]
    fn vm_file_backed_intrinsics_require_a_provider() {
        assert!(file_bytes_stream_value("data.bin", 4096).is_err());
        assert!(csv_rows_stream_value("data.csv").is_err());
        assert!(toml_parse_file_value("data.toml").is_err());
    }

    #[test]
    #[cfg(unix)]
    fn vm_process_request_requires_a_provider() {
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
        let error = process_run_request(&request).expect_err("provider-less spawn should fail");
        assert!(error.display().contains("external provider"));
    }

    #[test]
    #[cfg(unix)]
    fn vm_directory_listing_requires_a_provider() {
        let error = directory_list_files(std::path::Path::new("data"))
            .expect_err("provider-less listing should fail");
        assert!(error.to_string().contains("external provider"));
    }
}
