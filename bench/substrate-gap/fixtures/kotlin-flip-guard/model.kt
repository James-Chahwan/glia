package com.acme.model

import javax.persistence.Entity
import javax.persistence.Id

@Entity
class User {
    @Id
    var id: Long? = null
    var email: String = ""
}

@Entity
data class Order(@Id val id: Long, val total: Int)
