package main

import "fmt"

func helper(x int) int {
	return x * 2
}

func compute(n int) int {
	return helper(n) + 1
}

func main() {
	fmt.Println(compute(21))
}
