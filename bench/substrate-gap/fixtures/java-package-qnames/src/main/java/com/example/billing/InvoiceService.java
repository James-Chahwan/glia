package com.example.billing;

public class InvoiceService {
    public int total(int x) {
        return round(x) + 1;
    }

    private int round(int x) {
        return x;
    }

    public static class Row {}
}

class LineItem {
    void touch() {}
}
