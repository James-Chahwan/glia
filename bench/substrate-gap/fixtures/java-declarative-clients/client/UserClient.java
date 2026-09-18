package com.example.client;

import org.springframework.cloud.openfeign.FeignClient;
import org.springframework.web.bind.annotation.GetMapping;
import org.springframework.web.bind.annotation.PathVariable;
import org.springframework.web.service.annotation.GetExchange;
import org.springframework.web.service.annotation.HttpExchange;

@FeignClient(name = "users", url = "http://users-svc")
public interface UserClient {
    @GetMapping("/feign/users/{id}")
    String getUser(@PathVariable("id") long id);
}

@HttpExchange("/exchange")
interface ExchangeClient {
    @GetExchange("/users/{id}")
    String get(@PathVariable long id);
}
