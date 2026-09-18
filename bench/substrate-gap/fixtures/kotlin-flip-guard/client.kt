package com.acme.client

import org.springframework.web.client.RestTemplate

class UserClient {
    private val restTemplate = RestTemplate()

    fun fetch(): String {
        return restTemplate.getForObject("/api/users", String::class.java)
    }

    fun post(): String {
        val r = webClient.post().uri("/api/orders").retrieve()
        return "x"
    }
}
