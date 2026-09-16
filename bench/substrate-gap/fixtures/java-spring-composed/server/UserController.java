package com.example.server;

import org.springframework.web.bind.annotation.GetMapping;
import org.springframework.web.bind.annotation.PostMapping;
import org.springframework.web.bind.annotation.RequestMapping;
import org.springframework.web.bind.annotation.RestController;

@RestController
@RequestMapping("/api/v1/users")
public class UserController {

    @GetMapping("/{id}")
    public String getUser(String id) {
        return "user " + id;
    }

    @PostMapping
    public String createUser(String body) {
        return "created";
    }
}
