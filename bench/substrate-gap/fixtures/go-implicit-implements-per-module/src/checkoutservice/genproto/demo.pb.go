package hipstershop

type Empty struct{}
type AddItemRequest struct{ UserId string }
type GetCartRequest struct{ UserId string }
type Cart struct{ UserId string }
