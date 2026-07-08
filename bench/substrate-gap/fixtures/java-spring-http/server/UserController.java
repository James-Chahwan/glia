package com.example.server;

import org.springframework.web.bind.annotation.GetMapping;
import org.springframework.web.bind.annotation.PostMapping;
import org.springframework.web.bind.annotation.RestController;

@RestController
public class UserController {

    @GetMapping("/users/{id}")
    public String getUser(String id) {
        return "user " + id;
    }

    @PostMapping("/users")
    public String createUser(String body) {
        return "created";
    }
}
