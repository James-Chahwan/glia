package com.shop.web;

import com.shop.Color;

public class Picker {
    public Color choose() { return Color.pick(); }

    public Color green() { return Color.GREEN; }

    public String lbl(Color c) { return c.label(); }
}
