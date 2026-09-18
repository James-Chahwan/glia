package com.shop;

public enum Color {
    RED,
    GREEN("g") {
        @Override
        public String label() { return "green!"; }
    },
    BLUE;

    private final String code;

    Color() { this.code = ""; }

    Color(String c) { this.code = c; }

    public String label() { return code; }

    public static Color pick() { return RED; }

    public boolean warm() { return isRed(); }

    private boolean isRed() { return this == RED; }
}
