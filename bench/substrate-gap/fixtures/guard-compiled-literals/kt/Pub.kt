package pub

import org.springframework.kafka.core.KafkaTemplate

class Pub(private val kafkaTemplate: KafkaTemplate<String, String>) {
    // kafkaTemplate.send("ghost.kt", v)
    fun send(v: String) {
        kafkaTemplate.send("orders.kt", v)
    }
}
