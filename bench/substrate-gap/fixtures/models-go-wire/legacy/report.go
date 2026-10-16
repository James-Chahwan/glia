package legacy

import (
	"encoding/json"
	"net/http"

	"example.com/fleet/models"
)

func Report(w http.ResponseWriter, r *http.Request) {
	rep := models.Report{Vehicles: 3, Trips: 9}
	w.Header().Set("Content-Type", "application/json")
	_ = json.NewEncoder(w).Encode(rep)
}

func Mount(mux *http.ServeMux) {
	mux.HandleFunc("/report", Report)
}
