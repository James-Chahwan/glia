package com.example;

public class App {
    public int compute(int x) {
        return helper(x) + 1;
    }

    public int helper(int x) {
        return x * 2;
    }
}
