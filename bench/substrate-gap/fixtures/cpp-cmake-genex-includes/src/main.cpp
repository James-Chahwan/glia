#include <app/box.h>
#include <app/trace.h>

int main() {
    trace_on();
    return make_box().get();
}
