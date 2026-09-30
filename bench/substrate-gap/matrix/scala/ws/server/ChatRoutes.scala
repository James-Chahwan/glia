package shop

import akka.http.scaladsl.model.ws.Message
import akka.http.scaladsl.server.Directives._
import akka.stream.scaladsl.Flow

object ChatRoutes {
  val echo = Flow[Message]
  val route = path("ws" / "chat") {
    handleWebSocketMessages(echo)
  }
}
