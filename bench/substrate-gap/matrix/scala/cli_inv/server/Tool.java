package com.example;

import picocli.CommandLine.Command;

@Command(name = "invctl", subcommands = {Export.class})
public class Tool implements Runnable {
    public void run() {}
}

@Command(name = "export")
class Export implements Runnable {
    public void run() {}
}
