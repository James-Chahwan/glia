#pragma once
class Box {
public:
    int get() const;
    void put(int v);
    void clear();
};
Box make_box();
Box* find_box(int id);
