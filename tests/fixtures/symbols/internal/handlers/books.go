package handlers

import (
	"fmt"

	"example.com/app/internal/db"
)

func List(c *db.Client) {
	fmt.Println(db.Scoped(1), db.Raw) // easement:disable-line symbol -- legacy export, remove with #12
	db.Tx()
	db.Raw()
}
