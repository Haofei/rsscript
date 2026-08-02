## RSScript / REIR deployment review

Status: FAIL

### Required capabilities needing deployment grant

- s3:DeleteObject on arn:aws:s3:::reports-prod/*
  capability: object_storage.delete aws/s3 s3:DeleteObject arn:aws:s3:::reports-prod/*
  required by:
    - subject: Reports.cleanup_old_reports
      evidence: src/upload.rss:27 Reports.cleanup_old_reports -> S3.delete_object
    - subject: S3.delete_object
      evidence: interface/s3.rssi:9 S3.delete_object

### Current prod grants

- object_storage.write aws/s3 s3:PutObject arn:aws:s3:::reports-prod/*

### Review decision

- blocker: Evidence is invalid because diagnostic `fact.package.rss_s3_uploader_0_2_0.diagnostic.RS0030.2` reports an error. (fact.package.rss_s3_uploader_0_2_0.diagnostic.RS0030.2)
- blocker: Evidence is invalid because diagnostic `fact.package.rss_s3_uploader_0_2_0.diagnostic.RS0030.3` reports an error. (fact.package.rss_s3_uploader_0_2_0.diagnostic.RS0030.3)
- blocker: Evidence is invalid because diagnostic `fact.package.rss_s3_uploader_0_2_0.diagnostic.RS0206.0` reports an error. (fact.package.rss_s3_uploader_0_2_0.diagnostic.RS0206.0)
- blocker: Evidence is invalid because diagnostic `fact.package.rss_s3_uploader_0_2_0.diagnostic.RS0206.1` reports an error. (fact.package.rss_s3_uploader_0_2_0.diagnostic.RS0206.1)
- blocker: Evidence is invalid because diagnostic `fact.package.rss_s3_uploader_0_2_0.diagnostic.RS1301.4` reports an error. (fact.package.rss_s3_uploader_0_2_0.diagnostic.RS1301.4)
- blocker: Evidence is invalid because diagnostic `fact.package.rss_s3_uploader_0_2_0.diagnostic.RS1301.5` reports an error. (fact.package.rss_s3_uploader_0_2_0.diagnostic.RS1301.5)
- blocker: Required capability is not granted by the deployment target. (ObjectStorageDelete / aws / s3 / s3:DeleteObject / arn:aws:s3:::reports-prod/*)
