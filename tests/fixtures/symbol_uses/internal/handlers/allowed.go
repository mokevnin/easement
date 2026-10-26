package handlers

import "example.com/app/internal/db"

func Allowed() {
	db.Tx()
}
