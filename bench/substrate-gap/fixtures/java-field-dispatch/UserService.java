package com.example.svc;

import com.example.data.UserRepo;

public class UserService {
    private final UserRepo repo;
    private final AuditService audit;

    public UserService(UserRepo repo, AuditService audit) {
        this.repo = repo;
        this.audit = audit;
    }

    public String get(int id) {
        audit.log("x");
        return repo.find(id);
    }
}
