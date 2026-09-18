package com.shop;

import io.swagger.v3.oas.annotations.Operation;
import io.swagger.v3.oas.annotations.responses.ApiResponse;
import org.springframework.web.bind.annotation.*;

@RestController
@RequestMapping("/api/users")
public class UserController {
    @Operation(summary = "Get a user", operationId = "getUser")
    @ApiResponse(responseCode = "200", description = "found")
    @ApiResponse(responseCode = "404", description = "missing")
    @GetMapping("/{id}")
    public User getUser(@PathVariable String id) {
        return new User(id);
    }

    @PostMapping
    public User createUser(@RequestBody User user) {
        return user;
    }
}
