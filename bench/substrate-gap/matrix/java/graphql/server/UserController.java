package com.example.users;

import org.springframework.graphql.data.method.annotation.Argument;
import org.springframework.graphql.data.method.annotation.QueryMapping;
import org.springframework.stereotype.Controller;

@Controller
public class UserController {
    @QueryMapping
    public User user(@Argument String id) {
        return new User(id, "Ada");
    }

    @QueryMapping
    public User currentUser() {
        return new User("0", "me");
    }
}
