package services

import (
	"example.com/shop/internal/models"
	"example.com/shop/internal/repository"
)

func Books() []models.Book { return repository.All() }
