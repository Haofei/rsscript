resource "aws_iam_role_policy" "report_uploader" {
  role = "report-uploader"

  policy = <<POLICY
{
  "Version": "2012-10-17",
  "Statement": [
    {
      "Effect": "Allow",
      "Action": [
        "s3:PutObject",
        "s3:DeleteObject"
      ],
      "Resource": "arn:aws:s3:::reports-prod/*"
    }
  ]
}
POLICY
}
