# Excess IAM

Uses the base RSS package and `../../infra/terraform/excess/main.tf`.

Reviewer question: is this service overprivileged?

Expected result: RSScript requires `object_storage.write / s3:PutObject`, while the Terraform/OpenTofu IAM policy also grants `object_storage.delete / s3:DeleteObject`, so REIR reports an excess capability.
