#include "win.h"
void Base::Show() {}
class Win : public Base { public: void Run() { Show(); this->Show(); } };
