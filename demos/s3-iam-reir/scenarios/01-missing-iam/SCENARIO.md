# Missing IAM

Uses the base RSS package and `../../infra/mock-iam-missing.json`.

Reviewer question: would this deployment fail before the service reaches production?

Expected result: RSScript requires `object_storage.write / s3:PutObject`, but the mock IAM role grants only `object_storage.read / s3:GetObject`, so REIR reports a missing capability.
