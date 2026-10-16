package shop;

import org.apache.kafka.clients.producer.KafkaProducer;
import org.apache.kafka.clients.producer.ProducerRecord;

public class OrderProducer {
    public void send(KafkaProducer<String, String> p, String body) {
        p.send(new ProducerRecord<String, String>("orders", body));
    }
}
