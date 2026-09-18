package com.acme.web

import io.javalin.Javalin

fun main() {
    val app = Javalin.create()
    app.get("/users", UserHandler::list)
    app.post("/users", UserHandler::create)
}

fun routes(h: OrderHandler) = RouterFunctions.route()
    .GET("/orders", h::list)
    .POST("/orders", h::create)
    .build()
