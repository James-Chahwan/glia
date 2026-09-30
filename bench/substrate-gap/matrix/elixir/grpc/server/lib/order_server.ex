defmodule Shop.OrderService.Server do
  use GRPC.Server, service: Shop.OrderService.Service

  def get_order(request, _stream) do
    Shop.OrderReply.new(id: request.id)
  end
end
