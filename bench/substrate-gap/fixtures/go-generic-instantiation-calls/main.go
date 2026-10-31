package main

import "example.com/g/util"

func Keep[T any](x T) T { return x }

func id(x int) int { return x }

func main() {
	util.Map[int]([]int{1}, id)
	Keep[string]("a")
}
