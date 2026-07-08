trait Animal {
  def speak(): String
}

class Dog extends Animal {
  def speak(): String = "woof"
}
