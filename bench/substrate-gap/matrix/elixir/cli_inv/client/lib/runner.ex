defmodule Runner do
  def sync do
    System.cmd("mytool", ["sync"])
  end
end
