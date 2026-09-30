package com.acme.client

// Not a Ktor client file: a map's get / a store's post are no HTTP calls.
class Cache(private val entries: MutableMap<String, String>) {
    fun get(key: String): String? = entries.get("/api/orders")
    fun post(key: String, v: String) { entries.put(key, v) }
}
