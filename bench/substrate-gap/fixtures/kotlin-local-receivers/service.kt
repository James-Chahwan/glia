package com.acme.app

import com.acme.store.AuditLog
import com.acme.store.ItemStore

class Service {
    fun listAll(store: ItemStore): List<String> = store.all()

    fun audited(): String {
        val log: AuditLog = AuditLog()
        return log.write("typed")
    }

    fun inferred(): String {
        val log = AuditLog()
        return log.write("inferred")
    }

    fun shadowed(store: ItemStore): String {
        val log = store
        return log.write("aliased")
    }
}
