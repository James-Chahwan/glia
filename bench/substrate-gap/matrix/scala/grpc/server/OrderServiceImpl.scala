package shop

import scala.concurrent.Future
import shop.orders.{OrderReply, OrderRequest, OrderServiceGrpc}

class OrderServiceImpl extends OrderServiceGrpc.OrderService {
  override def getOrder(req: OrderRequest): Future[OrderReply] = Future.successful(OrderReply(req.id))
}
