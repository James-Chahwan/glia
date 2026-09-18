package com.example.web

import com.example.service.UserService
import org.springframework.web.bind.annotation.GetMapping
import org.springframework.web.bind.annotation.RestController
import org.springframework.web.bind.annotation.RequestMapping

@RestController
class UserController(private val userService: UserService) {
    @GetMapping("/users")
    fun list(): List<String> = userService.findAll()
}
