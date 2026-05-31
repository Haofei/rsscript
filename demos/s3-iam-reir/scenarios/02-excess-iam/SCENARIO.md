# Excess IAM

Uses the base RSS package and `../../infra/mock-iam-excess.json`.

Reviewer question: is this service overprivileged?

Expected result: RSScript requires `object_storage.write / s3:PutObject`, while the mock IAM role also grants `object_storage.delete / s3:DeleteObject`, so REIR reports an excess capability.
