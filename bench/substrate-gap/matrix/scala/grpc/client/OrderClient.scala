package shop

import io.grpc.ManagedChannelBuilder
import shop.orders.{OrderRequest, OrderServiceGrpc}

object OrderClient {
  def get() = {
    val channel = ManagedChannelBuilder.forAddress("localhost", 50051).usePlaintext().build()
    OrderServiceGrpc.stub(channel).getOrder(OrderRequest("1"))
  }
}
