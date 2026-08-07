#[cfg(test)]
mod resource_boundary_tests {
    use super::super::*;

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
}
