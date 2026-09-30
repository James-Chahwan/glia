use rumqttc::{AsyncClient, QoS};

pub async fn listen(client: &AsyncClient) {
    let _ = client.subscribe("sensors/temp", QoS::AtMostOnce).await;
}
