package com.acme.service

import com.acme.api.UserDto

open class BaseService {
    fun ping(): String = "pong"
}

class UserService : BaseService() {
    private val cache = mutableMapOf<Long, UserDto>()

    fun findById(id: Long): UserDto {
        return cache[id]!!
    }

    fun save(dto: UserDto): UserDto {
        cache[dto.id] = dto
        return dto
    }
}
