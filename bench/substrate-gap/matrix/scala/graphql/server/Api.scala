package shop

import caliban.graphQL
import caliban.RootResolver

case class Order(id: String)
case class Queries(orders: List[Order])

object Api {
  val api = graphQL(RootResolver(Queries(orders = Nil)))
}
