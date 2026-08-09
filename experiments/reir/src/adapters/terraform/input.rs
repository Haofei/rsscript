// Public source and plan limits accepted by the Terraform adapter.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TerraformPlanLimits {
    pub max_input_bytes: usize,
    pub max_json_depth: usize,
    pub max_json_nodes: usize,
    pub max_resources: usize,
    pub max_facts: usize,
}

impl Default for TerraformPlanLimits {
    fn default() -> Self {
        Self {
            max_input_bytes: 32 * 1024 * 1024,
            max_json_depth: 64,
            max_json_nodes: 1_000_000,
            max_resources: 100_000,
            max_facts: 250_000,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TerraformSourceLimits {
    pub max_files: usize,
    pub max_depth: usize,
    pub max_file_bytes: u64,
    pub max_total_bytes: u64,
}

impl Default for TerraformSourceLimits {
    fn default() -> Self {
        Self {
            max_files: 1_024,
            max_depth: 32,
            max_file_bytes: 2 * 1024 * 1024,
            max_total_bytes: 16 * 1024 * 1024,
        }
    }
}
