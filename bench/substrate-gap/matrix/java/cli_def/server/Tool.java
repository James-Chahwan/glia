package com.example;

import picocli.CommandLine;
import picocli.CommandLine.Command;

@Command(name = "invctl", mixinStandardHelpOptions = true, subcommands = {Export.class})
public class Tool implements Runnable {
    public void run() {}
}

@Command(name = "export", description = "Export inventory")
class Export implements Runnable {
    public void run() {}
}
