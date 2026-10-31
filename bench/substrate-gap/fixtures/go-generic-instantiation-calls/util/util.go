package util

func Map[T any](xs []T, f func(T) T) []T { return xs }
