package handlers

import (
	"fmt"

	"example.com/shop/internal/repository"
	"example.com/shop/internal/services"
)

func List() {
	fmt.Println(services.Books(), repository.All())
}
