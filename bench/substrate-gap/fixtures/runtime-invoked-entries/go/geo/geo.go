package geo

import "encoding/json"

var registry []string

// init runs when the package is linked: no source line calls it.
func init() { register("geo") }

func register(name string) { registry = append(registry, name) }

type Point struct{ X, Y int }

func NewPoint(x, y int) *Point {
	p := &Point{X: x, Y: y}
	p.Shift(0)
	q := &Queue{}
	q.Push(x)
	return p
}

func (p *Point) Shift(d int) { p.X += d }

// String satisfies fmt.Stringer: fmt calls it on a live Point.
func (p Point) String() string { return "point" }

// MarshalJSON satisfies json.Marshaler: encoding/json calls it.
func (p Point) MarshalJSON() ([]byte, error) { return json.Marshal([2]int{p.X, p.Y}) }

// Scan is named like sql.Scanner's but has another signature: nothing calls it.
func (p Point) Scan() bool { return p.X > 0 }

// Stringify only looks like String.
func (p Point) Stringify() string { return "" }

// A method named init is an ordinary method.
func (p Point) init() {}

// Orphan is never used, so its String is dead with it.
type Orphan struct{}

func (o Orphan) String() string { return "orphan" }

// Queue has a Len but no Less / Swap: it is no sort.Interface, so nothing calls Len.
type Queue struct{ items []int }

func (q *Queue) Push(v int) { q.items = append(q.items, v) }

func (q *Queue) Len() int { return len(q.items) }
