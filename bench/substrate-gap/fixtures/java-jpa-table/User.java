package com.example;

import javax.persistence.*;

@Entity
@Table(name = "app_users")
public class User {
    @Id
    private Long id;
    private String email;
}
