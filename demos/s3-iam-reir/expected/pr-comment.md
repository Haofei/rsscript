## RSScript / REIR deployment review

Status: FAIL

### Required capabilities needing deployment grant

- subject: Reports.cleanup_old_reports
  capability: object_storage.delete aws/s3 s3:DeleteObject arn:aws:s3:::reports-prod/*
  evidence: src/upload.rss:28 Reports.cleanup_old_reports -> S3.delete_object

### Current prod grants

- object_storage.write aws/s3 s3:PutObject arn:aws:s3:::reports-prod/*

### Missing capabilities

- s3:DeleteObject on arn:aws:s3:::reports-prod/*

### Review decision

Block this PR before deploy. Either remove the code paths above, or update IAM and review why the missing access is needed.
