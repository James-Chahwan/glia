package com.example;

import java.sql.Connection;
import java.sql.DriverManager;

public class Db {
    public Connection open() throws Exception {
        return DriverManager.getConnection(System.getenv("DATABASE_URL"));
    }
}
