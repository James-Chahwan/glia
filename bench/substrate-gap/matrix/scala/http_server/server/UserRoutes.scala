import cats.effect.IO
import org.http4s.{HttpRoutes, Response}
import org.http4s.dsl.io._

object UserRoutes {
  def getUser(id: String): IO[Response[IO]] = Ok(s"user $id")

  def createUser(): IO[Response[IO]] = Created("created")

  val routes: HttpRoutes[IO] = HttpRoutes.of[IO] {
    case GET -> Root / "users" / id => getUser(id)
    case POST -> Root / "users"     => createUser()
  }
}
