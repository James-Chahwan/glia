resource "aws_ecs_service" "api" {
  name            = "api"
  cluster         = aws_ecs_cluster.main.id
  task_definition = "api:1"
  desired_count   = 2
}

resource "aws_ecs_cluster" "main" {
  name = "prod"
}
