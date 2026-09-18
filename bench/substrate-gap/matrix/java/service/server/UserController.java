package com.example.users;

import java.util.List;
import org.springframework.web.bind.annotation.GetMapping;
import org.springframework.web.bind.annotation.RestController;

@RestController
public class UserController {
    private final UserService users = new UserService();

    @GetMapping("/users")
    public List<String> list() {
        return users.all();
    }
}
