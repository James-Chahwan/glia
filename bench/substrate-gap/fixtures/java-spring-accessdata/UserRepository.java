package com.example;

import javax.persistence.Entity;
import javax.persistence.Id;
import org.springframework.data.jpa.repository.JpaRepository;
import org.springframework.stereotype.Repository;

@Entity
class User {
    @Id
    private Long id;
    private String name;
}

@Repository
interface UserRepository extends JpaRepository<User, Long> {
    User findByName(String name);
}
