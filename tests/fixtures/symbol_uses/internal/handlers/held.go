package handlers

import "example.com/app/internal/db"

func Held() {
	held := db.Scoped(2)
	held.Query()
}
