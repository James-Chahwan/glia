package com.acme.api

import com.acme.service.UserService

interface Auditable {
    fun audit(): String
}

data class UserDto(val id: Long, val name: String)

@RestController
class UserController(private val userService: UserService) : Auditable {

    @GetMapping("/users/{id}")
    fun getUser(id: Long): UserDto = userService.findById(id)

    fun createUser(dto: UserDto): UserDto {
        val saved = userService.save(dto)
        logIt(saved)
        return saved
    }

    private fun logIt(dto: UserDto) {
        println(dto)
    }

    override fun audit(): String {
        return "ok"
    }
}

object Constants {
    const val MAX_USERS = 100
}

fun topLevelHelper(n: Int): Int = n * 2
