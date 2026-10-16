package models

type Vehicle struct {
	ID       string `json:"id"`
	Plate    string `json:"plate"`
	Odometer int    `json:"odometer"`
}

type Trip struct {
	ID        string  `json:"id"`
	VehicleID string  `json:"vehicleId"`
	Km        float64 `json:"km"`
}

type RegisterVehicle struct {
	Plate string `json:"plate" binding:"required"`
}

type Problem struct {
	Code    string `json:"code"`
	Message string `json:"message"`
}

type Report struct {
	Vehicles int `json:"vehicles"`
	Trips    int `json:"trips"`
}
