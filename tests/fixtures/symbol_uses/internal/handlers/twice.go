package handlers

import "example.com/app/internal/db"

type Pair struct {
	left  *db.Client
	right *db.Client
}
