resource "postgresql_grant" "report_writer_audit_events" {
  database    = "reports"
  role        = "report_writer"
  schema      = "public"
  object_type = "table"
  objects     = ["audit_events"]
  privileges  = ["SELECT"]
}
