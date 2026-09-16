package shop.notify;

import org.springframework.context.event.EventListener;
import org.springframework.stereotype.Component;

@Component
public class OrderEmailListener {
    @EventListener
    public void onOrderPlaced(OrderPlacedEvent event) {
        send(event);
    }

    private void send(OrderPlacedEvent event) {
    }
}
