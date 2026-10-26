package handlers

import "example.com/app/internal/db"

func Local() {
	var probe *db.Client
	_ = probe
}
