use tonic::{transport::Server, Request, Response, Status};

pub mod shop {
    tonic::include_proto!("shop");
}

use shop::order_service_server::{OrderService, OrderServiceServer};
use shop::{OrderReply, OrderRequest};

#[derive(Default)]
pub struct Orders;

#[tonic::async_trait]
impl OrderService for Orders {
    async fn get_order(&self, req: Request<OrderRequest>) -> Result<Response<OrderReply>, Status> {
        Ok(Response::new(OrderReply { id: req.into_inner().id }))
    }
}

#[tokio::main]
async fn main() {
    let _ = Server::builder().add_service(OrderServiceServer::new(Orders)).serve("[::1]:50051".parse().unwrap()).await;
}
