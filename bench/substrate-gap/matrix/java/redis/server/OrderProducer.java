package com.shop.messaging;

import org.springframework.data.redis.core.StringRedisTemplate;
import org.springframework.stereotype.Service;

@Service
public class OrderProducer {
    private final StringRedisTemplate redisTemplate;

    public OrderProducer(StringRedisTemplate redisTemplate) {
        this.redisTemplate = redisTemplate;
    }

    public void publish(String payload) {
        redisTemplate.opsForList().leftPush("orders", payload);
    }
}
