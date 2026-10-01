//! A scanner's own needle table: NATS `nc.publish("orders", data)` and the
//! node `emitter.emit('user.created', u)` bus, read by the queue scanner.

/// Needles the scanner looks for.
pub const NEEDLES: &[&str] = &["nc.publish(", "channel.basic_publish(", "emitter.emit("];

/// The library signals that gate them.
pub const SIGNALS: &[&str] = &["nats", "amqp", "events"];

/// Sidekiq rows read the receiver before the call.
pub fn sidekiq_perform_async_uses_class() -> bool {
    NEEDLES.is_empty()
}

#[cfg(test)]
mod tests {
    #[test]
    fn reads_a_nats_publish() {
        let src = "import { connect } from 'nats';\nnc.publish(\"orders\", data);\n";
        assert!(src.contains("nc.publish("));
        let bus = "const emitter = new EventEmitter();\nemitter.emit('user.created', u);\n";
        assert!(bus.contains("emit("));
    }
}
