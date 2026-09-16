package clients

import play.api.libs.ws.WSClient
import sttp.client3._

// Play WS + sttp client calls -> ENDPOINT nodes that pair with the Go server.
class ApiClient(ws: WSClient, config: Config) {

  // Play WS fluent chain: the URL is on `.url(...)`, the verb on `.get()`.
  def fetchUser(id: String) = ws.url(s"http://users-svc/api/users/$id").get()

  // Verb is `.post(...)`, URL is a plain relative literal.
  def createUser(body: String) = ws.url("/api/users").post(body)

  // sttp: the path lives in a `uri"…"` interpolator on the verb call itself.
  def listOrders() = basicRequest.get(uri"http://orders-svc/api/orders")

  // Negative control: `.get` on a non-WS receiver with a non-URL string.
  def cached(id: String) = config.get("user." + id)
}
