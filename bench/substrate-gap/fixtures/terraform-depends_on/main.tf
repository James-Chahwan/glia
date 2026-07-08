resource "aws_iam_role" "app" {
  name = "app-role"
}

resource "aws_s3_bucket" "data" {
  bucket = "my-app-data"
}

resource "aws_instance" "worker" {
  ami           = "ami-123456"
  instance_type = "t3.micro"

  depends_on = [
    aws_iam_role.app,
    aws_s3_bucket.data,
  ]
}
