use rumqttc::{AsyncClient, QoS};

pub async fn report(client: &AsyncClient) {
    let _ = client.publish("sensors/temp", QoS::AtLeastOnce, false, "21").await;
}
