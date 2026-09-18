package com.acme.client

import retrofit2.http.GET
import retrofit2.http.POST

interface UserApi {
    @GET("/api/users")
    suspend fun listUsers(): List<String>

    @POST("/api/users")
    suspend fun createUser(name: String): String
}
