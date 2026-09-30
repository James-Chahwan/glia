#include "../include/widget.hh"
#include "detail.inl"
#include "impl.ipp"
#include "pair.hxx"

void Widget::run() {
    this->size();
    detail_helper();
    impl_twice(2);
}
