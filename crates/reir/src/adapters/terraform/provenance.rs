// Complete producer provenance for source and plan adapters.

fn terraform_source_provenance() -> ProducerProvenance {
    ProducerProvenance {
        name: "terraform",
        version: PRODUCER_VERSION,
        adapter: "reir.adapters.terraform",
        adapter_version: ADAPTER_VERSION,
        source: PRODUCER_SOURCE,
    }
}

fn terraform_plan_provenance() -> ProducerProvenance {
    ProducerProvenance {
        name: "terraform-plan",
        version: PRODUCER_VERSION,
        adapter: "reir.adapters.terraform_plan",
        adapter_version: ADAPTER_VERSION,
        source: "terraform_plan_json",
    }
}
