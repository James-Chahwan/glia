require "grpc"
require "orders_services_pb"

class OrderServer < Shop::OrderService::Service
  def get_order(req, _call)
    Shop::OrderReply.new(id: req.id)
  end
end

s = GRPC::RpcServer.new
s.add_http2_port("0.0.0.0:50051", :this_port_is_insecure)
s.handle(OrderServer)
