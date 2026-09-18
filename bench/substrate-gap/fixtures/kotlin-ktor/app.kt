package com.acme.ktor

import com.acme.store.ItemStore

fun Application.itemRoutes(store: ItemStore) {
    routing {
        get("/api/items") {
            call.respond(store.all())
        }
        post("/api/items") {
            call.respond(store.add("x"))
        }
    }
}
