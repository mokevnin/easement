package billing

import (
	"testing"

	"example.com/app/internal/store"
)

func TestCharge(t *testing.T) {
	var client *store.Client
	Charge(client)
}
