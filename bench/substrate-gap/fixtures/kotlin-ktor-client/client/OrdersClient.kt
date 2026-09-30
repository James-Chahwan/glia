package com.acme.client

import io.ktor.client.HttpClient
import io.ktor.client.call.body
import io.ktor.client.request.get
import io.ktor.client.request.post
import io.ktor.client.request.setBody

class OrdersClient(private val client: HttpClient) {
    suspend fun listOrders(): String = client.get("http://orders:8080/api/orders").body()

    suspend fun createOrder(body: String): String =
        client.post("/api/orders") { setBody(body) }.body()

    suspend fun getOrder(id: String): String = client.get("/api/orders/$id").body()
}
