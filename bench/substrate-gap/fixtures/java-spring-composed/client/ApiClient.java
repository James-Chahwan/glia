package com.example.client;

import org.springframework.web.client.RestTemplate;

public class ApiClient {
    private final RestTemplate rest = new RestTemplate();

    public String fetchUser(String id) {
        return rest.getForObject("/api/v1/users/" + id, String.class);
    }

    public String createUser(String body) {
        return rest.postForObject("/api/v1/users", body, String.class);
    }
}
