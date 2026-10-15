pub struct Notifier;

impl Notifier {
    pub fn publish(&self, _topic: &str) {}
}

pub fn notify(n: &Notifier) {
    n.publish("audit");
}
