package shop

class Widget {
  def run(): Int = helper()
  def helper(): Int = 1
}

object Widget {
  def make(): Widget = new Widget()
}

trait Gadget {
  def go(): Unit
}

def topLevel(): Int = 3
