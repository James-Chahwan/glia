package com.acme.server

import io.ktor.server.application.Application
import io.ktor.server.response.respond
import io.ktor.server.routing.get
import io.ktor.server.routing.post
import io.ktor.server.routing.routing

fun Application.orderRoutes() {
    routing {
        get("/api/orders") { call.respond(listOf<String>()) }
        post("/api/orders") { call.respond("ok") }
        get("/api/orders/{id}") { call.respond("one") }
    }
}
