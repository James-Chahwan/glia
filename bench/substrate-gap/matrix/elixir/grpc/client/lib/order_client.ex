defmodule OrderClient do
  def get do
    {:ok, channel} = GRPC.Stub.connect("localhost:50051")
    Shop.OrderService.Stub.get_order(channel, Shop.OrderRequest.new(id: "1"))
  end
end
