import cats.effect.IO
import org.http4s.{HttpRoutes, Response}
import org.http4s.dsl.io._

object UserRoutes {
  def getUser(id: String): IO[Response[IO]] = Ok(s"user $id")

  val routes: HttpRoutes[IO] = HttpRoutes.of[IO] {
    case GET -> Root / "users" / id => getUser(id)
  }
}
