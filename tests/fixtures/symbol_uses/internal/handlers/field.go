package handlers

import "example.com/app/internal/db"

type Repo struct {
	client *db.Client
}
