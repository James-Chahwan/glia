pub mod shop {
    tonic::include_proto!("shop");
}

use shop::order_service_client::OrderServiceClient;

pub async fn get() {
    let mut client = OrderServiceClient::connect("http://[::1]:50051").await.unwrap();
    let _ = client.get_order(shop::OrderRequest { id: "1".into() }).await;
}
