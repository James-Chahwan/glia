package com.example;

public class Exporter {
    public int run() throws Exception {
        Process p = new ProcessBuilder("invctl", "export").inheritIO().start();
        return p.waitFor();
    }
}
