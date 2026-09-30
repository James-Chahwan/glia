use azservicebus::prelude::*;

pub async fn send(conn: &str, body: &str) -> Result<(), Box<dyn std::error::Error>> {
    let mut client = ServiceBusClient::new_from_connection_string(conn, ServiceBusClientOptions::default()).await?;
    let mut sender = client.create_sender("orders", ServiceBusSenderOptions::default()).await?;
    let mut batch = sender.create_message_batch(Default::default())?;
    batch.try_add_message(ServiceBusMessage::new(body.to_string()))?;
    sender.send_message_batch(batch).await?;
    Ok(())
}
