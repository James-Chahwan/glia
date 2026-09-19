class Animal {
  void speak() {}
}

abstract class Walker {
  void walk();
}

mixin Runs {
  void run() {}
}

class Cat extends Animal {
  void purr() {}
}
