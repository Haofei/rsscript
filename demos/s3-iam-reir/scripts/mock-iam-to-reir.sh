#!/usr/bin/env sh
set -eu

mode="${1:-missing}"
case "$mode" in
  missing)
    action="s3:GetObject"
    source_file="infra/mock-iam-missing.json"
    ;;
  fixed)
    action="s3:PutObject"
    source_file="infra/mock-iam-fixed.json"
    ;;
  *)
    echo "usage: $0 [missing|fixed]" >&2
    exit 2
    ;;
esac

cat <<JSON
{
  "schema": "reir.bundle.v0.1",
  "ontology": "reir.capability_ontology.v0.1",
  "facts": [
    {
      "schema": "reir.fact.v0.1",
      "id": "fact.mock_runtime.native_allowed",
      "kind": "capability",
      "role": "granted",
      "subject": {
        "kind": "service",
        "id": "prod/report-uploader",
        "name": "report-uploader"
      },
      "capability": {
        "category": "runtime.native",
        "provider": "rsscript"
      },
      "value": true,
      "confidence": {
        "level": "authoritative",
        "source": "mock_runtime"
      },
      "acquisition_mode": "rendered_manifest",
      "precision": "category",
      "evidence": [
        {
          "kind": "rendered_manifest_pointer",
          "file": "infra/mock-runtime.json",
          "source": "mock_runtime"
        }
      ]
    },
    {
      "schema": "reir.fact.v0.1",
      "id": "fact.mock_iam.report_uploader.${mode}",
      "kind": "capability",
      "role": "granted",
      "subject": {
        "kind": "cloud.role",
        "id": "arn:aws:iam::123456789012:role/report-uploader",
        "name": "report-uploader"
      },
      "capability": {
        "category": "object_storage.write",
        "provider": "aws",
        "service": "s3",
        "action": "${action}",
        "resource": "arn:aws:s3:::reports-prod/*"
      },
      "value": true,
      "confidence": {
        "level": "authoritative",
        "source": "mock_iam"
      },
      "acquisition_mode": "cloud_policy",
      "precision": "resource_scoped",
      "evidence": [
        {
          "kind": "cloud_policy_pointer",
          "file": "${source_file}",
          "resource": "arn:aws:iam::123456789012:role/report-uploader",
          "provider": "aws",
          "source": "mock_iam",
          "action": "${action}"
        }
      ]
    }
  ]
}
JSON
