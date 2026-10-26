package handlers

import "example.com/app/internal/db"

func Chained() {
	db.Scoped(1).Query()
}
