# Missing IAM

Uses the base RSS package and `../../infra/terraform/missing/main.tf`.

Reviewer question: would this deployment fail before the service reaches production?

Expected result: RSScript requires `object_storage.write / s3:PutObject`, but the Terraform/OpenTofu IAM policy grants only `object_storage.read / s3:GetObject`, so REIR reports a missing capability.
