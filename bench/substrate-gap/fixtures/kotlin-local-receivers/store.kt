package com.acme.store

class ItemStore {
    fun all(): List<String> = listOf()
    fun write(s: String): String = s
}

class AuditLog {
    fun write(s: String): String = s
}
