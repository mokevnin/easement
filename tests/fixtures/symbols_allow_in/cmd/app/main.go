package main

import (
	"example.com/app/internal/modules/billing"
	"example.com/app/internal/store"
)

func main() {
	var client *store.Client = store.Open()
	billing.Charge(client)
}
