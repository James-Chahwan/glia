package com.acme.lib;

public class Greeter {
    public String greet(String name) {
        return "hello " + name;
    }

    public static String shout(String name) {
        return name.toUpperCase();
    }
}
