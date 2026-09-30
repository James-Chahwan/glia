lazy val api = project.in(file("api"))
lazy val worker = project.in(file("worker"))
lazy val root = project.in(file(".")).aggregate(api, worker)
