package com.acme.web

@RestController
@RequestMapping("/api")
class UserController {
    @GetMapping("/users")
    fun list(): List<String> = listOf()
}
