package com.shop.workers;

import java.time.Duration;
import org.springframework.data.redis.core.StringRedisTemplate;
import org.springframework.stereotype.Component;

@Component
public class OrderConsumer {
    private final StringRedisTemplate redisTemplate;

    public OrderConsumer(StringRedisTemplate redisTemplate) {
        this.redisTemplate = redisTemplate;
    }

    public void poll() {
        String payload = redisTemplate.opsForList().rightPop("orders", Duration.ofSeconds(5));
        System.out.println(payload);
    }
}
