require "grpc"
require "orders_services_pb"

stub = Shop::OrderService::Stub.new("localhost:50051", :this_channel_is_insecure)
stub.get_order(Shop::OrderRequest.new(id: "1"))
