package com.example;

public abstract class Shape {
    public abstract double area();
}

interface Drawable {
    void draw();
}

class Circle extends Shape implements Drawable {
    private double r;
    public double area() { return 3.14 * r * r; }
    public void draw() {}
}
